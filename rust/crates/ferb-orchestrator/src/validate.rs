//! The answer models' validation and dumps: `<model>_validate` (pydantic's `Model.model_validate(value)`),
//! `<model>_validate_json` (`Model.model_validate_json(text)`), `dump_<model>` (`model_dump()`, as `Py` in field
//! order) and `dump_json_<model>` (`model_dump_json()`). The class definitions, defaults and validators are those of
//! ferb/orchestrator/schemas.py, plans.py, conversations.py, probes.py and ferb/clock/when.py; the engine with
//! pydantic's coercions and error text is `crate::pydantic`. Fixture: `rust/fixtures/orchestrator/models.json`.
//!
//! decision: ORCH-630, ORCH-582, ORCH-545, ORCH-604, ORCH-319, ORCH-322

use ferb_context::pyvalue::{Dict, List, Py};
use indexmap::IndexMap;

use crate::pydantic::{
    dump_json, get, is_str, iterate, list_or_empty, star, truthy, unique, validate_model,
    validate_model_json, After, Before, Field, ModelDef, T,
};
pub use crate::pydantic::{ErrorItem, Loc, Raised, ValidationError};
use crate::*;

// ---------------------------------------------------------------------------------------------------------------
// Class definitions
// ---------------------------------------------------------------------------------------------------------------

const IDENTITY: &[&str] = &["owner", "ferb"];
const CHECK_KINDS: &[&str] = &[
    "none",
    "file_exists",
    "file_contains",
    "tab_url",
    "account",
    "after_change",
    "after_value",
    "sent",
    "paid",
    "submitted",
    "trashed",
    "deleted",
    "downloaded",
];

fn req(name: &'static str, t: T) -> Field {
    Field {
        name,
        t,
        default: None,
    }
}

fn opt(name: &'static str, t: T, default: fn() -> Py) -> Field {
    Field {
        name,
        t,
        default: Some(default),
    }
}

fn s() -> T {
    T::Str(0)
}
fn os() -> T {
    T::Opt(Box::new(T::Str(0)))
}
fn ls() -> T {
    T::List(Box::new(T::Str(0)), 0)
}
fn lm(name: &'static str) -> T {
    T::List(Box::new(T::Model(name)), 0)
}
fn om(name: &'static str) -> T {
    T::Opt(Box::new(T::Model(name)))
}
fn lit(allowed: &'static [&'static str]) -> T {
    T::Lit(allowed)
}

fn none() -> Py {
    Py::None
}
fn empty() -> Py {
    Py::Str(String::new())
}
fn nothing() -> Py {
    Py::List(List(Vec::new()))
}
fn no() -> Py {
    Py::Bool(false)
}
fn yes() -> Py {
    Py::Bool(true)
}
fn zero() -> Py {
    Py::Int("0".into())
}
fn default_word() -> Py {
    Py::Str("default".into())
}
fn targets_default() -> Py {
    dict(vec![
        ("apps", nothing()),
        ("sites", nothing()),
        ("paths", nothing()),
        ("recipients", nothing()),
    ])
}
fn timing_default() -> Py {
    dict(vec![
        ("repeat", empty()),
        ("times", nothing()),
        ("every_minutes", zero()),
        ("weekday", empty()),
        ("random", nothing()),
    ])
}

fn model(
    name: &'static str,
    fields: Vec<Field>,
    befores: Vec<Before>,
    after: Option<After>,
) -> ModelDef {
    ModelDef {
        name,
        fields,
        befores,
        after,
    }
}

pub(crate) fn models() -> Vec<ModelDef> {
    vec![
        model(
            "When",
            vec![
                opt("in_minutes", T::Opt(Box::new(T::Float)), none),
                opt("in_days", T::Opt(Box::new(T::Int)), none),
                opt("day", os(), none),
                opt("date", os(), none),
                opt("time", os(), none),
            ],
            vec![],
            None,
        ),
        model(
            "Probe",
            vec![
                req("tool", s()),
                opt("args_json", s(), || Py::Str("{}".into())),
                req("field", s()),
                req("op", lit(&["equals", "not_equals", "changed"])),
                opt("value_json", s(), || Py::Str("null".into())),
            ],
            vec![],
            None,
        ),
        model(
            "ConversationScope",
            vec![
                req("identity", lit(IDENTITY)),
                req("account", T::Str(1)),
                req("site", T::Str(1)),
                req("app", T::Str(1)),
                req("thread", T::Str(1)),
                req("recipients", T::List(Box::new(T::Str(0)), 1)),
                req("topic", T::Str(1)),
                req("handover_when", T::Str(1)),
                opt("window_id", os(), none),
            ],
            vec![],
            Some(concrete),
        ),
        model(
            "EventMatch",
            vec![req("field", s()), req("value", s())],
            vec![],
            None,
        ),
        model(
            "Repeat",
            vec![
                opt("end_when", s(), empty),
                opt("event", os(), none),
                opt("event_match", lm("EventMatch"), nothing),
            ],
            vec![],
            None,
        ),
        model(
            "Step",
            vec![
                req("id", s()),
                req("kind", lit(&["do", "wait", "ask", "converse"])),
                opt("label", s(), empty),
                opt("goal", s(), empty),
                opt("depends_on", ls(), nothing),
                opt("inputs", ls(), nothing),
                opt("worker_type", s(), default_word),
                opt("conditions", ls(), nothing),
                opt("until", os(), none),
                opt("event", os(), none),
                opt("event_match", lm("EventMatch"), nothing),
                opt("conversation", om("ConversationScope"), none),
                opt("question", s(), empty),
                opt("options", ls(), nothing),
                opt("repeat", om("Repeat"), none),
                opt("earlier_results", ls(), nothing),
            ],
            vec![earlier_kinds],
            None,
        ),
        model(
            "SendScope",
            vec![
                req("capability", s()),
                req("identity", lit(IDENTITY)),
                opt("account", os(), none),
                opt("site", os(), none),
                opt("recipients", ls(), nothing),
                opt("content", os(), empty),
            ],
            vec![],
            None,
        ),
        model(
            "Part",
            vec![
                req("id", s()),
                req("quote", s()),
                req("label", s()),
                opt("direct", os(), none),
                opt("answer_step", os(), none),
                opt("conditions", T::List(Box::new(T::Int), 0), nothing),
            ],
            vec![],
            None,
        ),
        model(
            "Plan",
            vec![
                req("steps", lm("Step")),
                opt("input_paths", ls(), nothing),
                opt("big_job", T::Bool, no),
                opt("time_estimate", s(), empty),
                opt("cost_estimate", s(), empty),
                opt("standing_sends", lm("SendScope"), nothing),
            ],
            vec![],
            None,
        ),
        model(
            "TargetsSpec",
            vec![
                opt("apps", ls(), nothing),
                opt("sites", ls(), nothing),
                opt("paths", ls(), nothing),
                opt("recipients", ls(), nothing),
            ],
            vec![],
            None,
        ),
        model(
            "Check",
            vec![
                req("kind", lit(CHECK_KINDS)),
                req("path", os()),
                req("text", os()),
                req("url_contains", os()),
                req("site", os()),
                req("expected", os()),
                opt("tool", os(), none),
            ],
            vec![legacy_ledger_done],
            None,
        ),
        model(
            "Condition",
            vec![
                req("text", s()),
                opt("owner_text", s(), empty),
                req("check", T::Model("Check")),
                opt(
                    "layer",
                    lit(&["outcome", "intake", "artifact", "external"]),
                    || Py::Str("outcome".into()),
                ),
            ],
            vec![no_code_check],
            None,
        ),
        model(
            "TaskSpec",
            vec![
                req("goal", s()),
                opt("name", s(), empty),
                req("identity", lit(IDENTITY)),
                opt("worker_type", s(), default_word),
                opt("targets", T::Model("TargetsSpec"), targets_default),
                opt("success_conditions", lm("Condition"), nothing),
                opt("plan", om("Plan"), none),
                opt("folders", ls(), nothing),
                opt("keep_tabs_open", T::Bool, no),
                opt("parts", lm("Part"), nothing),
                opt("earlier_results", ls(), nothing),
            ],
            vec![],
            None,
        ),
        model(
            "Until",
            vec![
                req("quote", s()),
                opt("day", os(), none),
                opt("date", os(), none),
                opt("in_days", T::Opt(Box::new(T::Int)), none),
            ],
            vec![],
            None,
        ),
        model(
            "ScheduleUntil",
            vec![
                req("quote", s()),
                opt("day", os(), none),
                opt("date", os(), none),
                opt("in_days", T::Opt(Box::new(T::Int)), none),
                opt("time", os(), none),
                opt("in_minutes", T::Opt(Box::new(T::Float)), none),
            ],
            vec![unused],
            None,
        ),
        model(
            "Remember",
            vec![
                req(
                    "kind",
                    lit(&["fact", "preference", "person", "project", "env"]),
                ),
                req("subject", s()),
                req("key", s()),
                req("content", s()),
                req("quote", s()),
                opt("login_origin", os(), none),
                opt("until", om("Until"), none),
                opt("replaces", ls(), nothing),
                opt("review_in_days", T::Int, zero),
            ],
            vec![],
            None,
        ),
        model(
            "ForgetRequest",
            vec![req("item_id", s()), req("quote", s())],
            vec![],
            None,
        ),
        model(
            "AnswerQuestion",
            vec![req("approval_id", s()), req("answer", s())],
            vec![],
            None,
        ),
        model(
            "TaskControl",
            vec![
                req("task_id", s()),
                req("action", lit(&["stop", "retry", "continue", "pause"])),
                opt("new_goal", T::Bool, no),
                req(
                    "standing_sends",
                    T::Union(vec![
                        ("literal['unchanged']", lit(&["unchanged"])),
                        ("list[SendScope]", lm("SendScope")),
                    ]),
                ),
                req("supersedes", ls()),
            ],
            vec![],
            None,
        ),
        model(
            "SupervisorOutcome",
            vec![
                opt("control", om("TaskControl"), none),
                req("kind", lit(&["reply", "clarify", "task"])),
                req("text", s()),
                opt("answers_question", T::Bool, yes),
                opt("answer_question", om("AnswerQuestion"), none),
                opt("task", om("TaskSpec"), none),
                opt("remember", lm("Remember"), nothing),
                opt("forget", lm("ForgetRequest"), nothing),
                opt("timers", lm("TimerRequest"), nothing),
                opt("schedules", lm("ScheduleRequest"), nothing),
                opt("schedule_changes", lm("ScheduleChange"), nothing),
                opt("show_schedules", T::Bool, no),
                opt("reminder_cancels", lm("ReminderCancel"), nothing),
                opt("conversation_changes", lm("ConversationChange"), nothing),
            ],
            // pydantic runs the later-defined before-validator first: _entries_only, then _one_schedule
            vec![entries_only, one_schedule],
            Some(complete_action_plan),
        ),
        model(
            "RandomTimes",
            vec![req("count", T::Int), req("start", s()), req("end", s())],
            vec![],
            None,
        ),
        model(
            "ScheduleTiming",
            vec![
                opt(
                    "repeat",
                    lit(&["", "none", "daily", "weekdays", "weekly", "every"]),
                    empty,
                ),
                opt("times", ls(), nothing),
                opt("every_minutes", T::Int, zero),
                opt("weekday", s(), empty),
                opt("random", lm("RandomTimes"), nothing),
            ],
            vec![],
            None,
        ),
        model(
            "ScheduleRequest",
            vec![
                req("request", s()),
                opt("name", s(), empty),
                req("when", T::Model("When")),
                req(
                    "repeat",
                    lit(&["none", "daily", "weekdays", "weekly", "every"]),
                ),
                opt("weekday", os(), none),
                opt("every_minutes", T::Opt(Box::new(T::Int)), none),
                opt("times", ls(), nothing),
                opt("random", lm("RandomTimes"), nothing),
                req("quote", s()),
                opt("targets", T::Model("TargetsSpec"), targets_default),
                opt("standing_sends", lm("SendScope"), nothing),
                opt("watch", lit(&["none", "until_met", "standing"]), || {
                    Py::Str("none".into())
                }),
                opt("until", om("ScheduleUntil"), none),
                opt("when_active", T::Bool, no),
                opt("folders", ls(), nothing),
                opt("identity", T::Opt(Box::new(lit(IDENTITY))), none),
                opt("worker_type", s(), empty),
                opt("profile", s(), empty),
                opt("account", s(), empty),
            ],
            vec![],
            None,
        ),
        model(
            "ScheduleChange",
            vec![
                req("id", s()),
                req(
                    "action",
                    lit(&["cancel", "pause", "resume", "set_end", "edit"]),
                ),
                req("quote", s()),
                opt("request", s(), empty),
                opt("timing", T::Model("ScheduleTiming"), timing_default),
                opt("until", om("ScheduleUntil"), none),
                opt("name", s(), empty),
            ],
            vec![],
            None,
        ),
        model(
            "ReminderCancel",
            vec![req("id", s()), req("quote", s())],
            vec![],
            None,
        ),
        model(
            "TimerRequest",
            vec![
                req("when", T::Model("When")),
                req("message", s()),
                req("quote", s()),
                opt("when_active", T::Bool, no),
                opt("name", s(), empty),
                opt("kind", lit(&["reminder", "timer"]), || {
                    Py::Str("reminder".into())
                }),
            ],
            vec![],
            None,
        ),
        model(
            "ConversationChange",
            vec![
                req("id", s()),
                req("action", lit(&["pause", "resume", "takeover", "close"])),
                req("quote", s()),
            ],
            vec![],
            None,
        ),
        model(
            "WorkerReport",
            vec![
                req("status", lit(&["done", "not_done"])),
                opt("reason", s(), empty),
                opt("question", os(), none),
                opt("answer", s(), empty),
                opt("artifacts", ls(), nothing),
                opt("how", s(), empty),
                opt("input_notes", T::List(Box::new(T::Dict), 0), nothing),
                opt(
                    "conversation_action",
                    T::Opt(Box::new(lit(&["wait", "handover", "finished", "blocked"]))),
                    none,
                ),
                opt(
                    "conversation_proposals",
                    T::List(Box::new(T::Dict), 0),
                    nothing,
                ),
                opt("watch_met", T::Opt(Box::new(T::Bool)), none),
                opt(
                    "loop_action",
                    T::Opt(Box::new(lit(&["continue", "finished", "blocked"]))),
                    none,
                ),
                opt("probe", om("Probe"), none),
                opt("part_updates", T::List(Box::new(T::Dict), 0), nothing),
            ],
            vec![],
            Some(worker_tags),
        ),
        model(
            "QuestionOutcome",
            vec![
                req("ask_owner", T::Bool),
                req("answer", s()),
                req("question", s()),
            ],
            vec![],
            None,
        ),
        model(
            "CheckerResult",
            vec![
                req("index", T::Int),
                req("met", T::Opt(Box::new(T::Bool))),
                opt("applies", T::Bool, yes),
                req("evidence", s()),
                opt("how", os(), none),
            ],
            vec![],
            None,
        ),
        model(
            "Finding",
            vec![
                req("quote", s()),
                req("kind", lit(&["contradicted", "unsupported"])),
                opt("evidence", s(), empty),
            ],
            vec![],
            None,
        ),
        model("FixOutcome", vec![req("answer", s())], vec![], None),
        model(
            "RecheckOutcome",
            vec![opt("findings", lm("Finding"), nothing)],
            vec![],
            None,
        ),
        model("SummaryOutcome", vec![req("summary", s())], vec![], None),
        model(
            "CheckerOutcome",
            vec![
                req("results", lm("CheckerResult")),
                req(
                    "next",
                    lit(&["accept", "retry", "ask", "accept_unconfirmed", "fail"]),
                ),
                opt("findings", lm("Finding"), nothing),
                opt("missing", s(), empty),
                opt("instructions", s(), empty),
                opt("worker_type", os(), none),
                opt("question", s(), empty),
                opt("reason", s(), empty),
                opt("dropped_edits", ls(), nothing),
            ],
            vec![older_shape],
            Some(checker_tags),
        ),
    ]
}

// ---------------------------------------------------------------------------------------------------------------
// Validators
// ---------------------------------------------------------------------------------------------------------------

/// A dict's entries, owned; anything else back as it was.
fn take_map(data: Py) -> Result<IndexMap<String, Py>, Py> {
    match data {
        Py::Dict(mut d) => Ok(std::mem::take(&mut d.0)),
        other => Err(other),
    }
}

fn map_of(data: &mut Py) -> Option<&mut IndexMap<String, Py>> {
    match data {
        Py::Dict(d) => Some(&mut d.0),
        _ => None,
    }
}

/// schemas.py:367 `Check._legacy_ledger_done`.
fn legacy_ledger_done(mut data: Py) -> Result<Py, Raised> {
    if is_str(get(&data, "kind"), "ledger_done") {
        if let Some(m) = map_of(&mut data) {
            m.insert("kind".into(), Py::Str("none".into()));
        }
    }
    Ok(data)
}

fn no_check() -> Py {
    dict(vec![
        ("kind", Py::Str("none".into())),
        ("path", Py::None),
        ("text", Py::None),
        ("url_contains", Py::None),
        ("site", Py::None),
        ("expected", Py::None),
        ("tool", Py::None),
    ])
}

/// schemas.py:387 `Condition._no_code_check`.
fn no_code_check(mut data: Py) -> Result<Py, Raised> {
    if let Some(m) = map_of(&mut data) {
        if matches!(m.get("check"), None | Some(Py::None)) {
            m.insert("check".into(), no_check());
        }
    }
    Ok(data)
}

/// schemas.py:426 `ScheduleUntil._unused`: `v in ("", 0)` is None.
fn unused(mut data: Py) -> Result<Py, Raised> {
    if let Some(m) = map_of(&mut data) {
        for (k, v) in m.iter_mut() {
            let blank = match v {
                Py::Str(s) => s.is_empty(),
                Py::Int(i) => i == "0",
                Py::Float(f) => *f == 0.0,
                Py::Bool(b) => !*b,
                _ => false,
            };
            if k != "quote" && blank {
                *v = Py::None;
            }
        }
    }
    Ok(data)
}

/// plans.py:57 `Step._earlier_kinds`.
fn earlier_kinds(data: Py) -> Result<Py, Raised> {
    let kind = match get(&data, "kind") {
        Some(Py::Str(k)) if k == "map" || k == "reduce" || k == "loop" => k.clone(),
        _ => return Ok(data),
    };
    let Ok(mut m) = take_map(data) else {
        unreachable!("a dict")
    };
    m.shift_remove("kind");
    m.insert("kind".into(), Py::Str("do".into()));
    let batches = m
        .shift_remove("batches")
        .filter(truthy)
        .unwrap_or(Py::List(List(Vec::new())));
    if kind == "map" {
        let mut items = match m.get("inputs") {
            Some(v) => star(v)?,
            None => Vec::new(),
        };
        for batch in iterate(&batches)? {
            items.extend(iterate(&batch)?);
        }
        m.insert("inputs".into(), Py::List(List(unique(items)?)));
    }
    if kind == "loop" {
        let end_when = m
            .shift_remove("end_when")
            .filter(truthy)
            .unwrap_or(Py::Str(String::new()));
        let event = m.shift_remove("event").unwrap_or(Py::None);
        let event_match = m
            .shift_remove("event_match")
            .unwrap_or(Py::List(List(Vec::new())));
        m.insert(
            "repeat".into(),
            dict(vec![
                ("end_when", end_when),
                ("event", event),
                ("event_match", event_match),
            ]),
        );
    }
    Ok(Py::Dict(Dict(m)))
}

const SIDE_ENTRIES: &[&str] = &[
    "answer_question",
    "remember",
    "forget",
    "timers",
    "schedules",
    "schedule_changes",
    "show_schedules",
    "reminder_cancels",
    "conversation_changes",
];

/// schemas.py:227 `ungrouped`.
pub fn ungrouped(answer: Py) -> Py {
    let m = match take_map(answer) {
        Ok(m) => m,
        Err(other) => return other,
    };
    if !m.contains_key("side_entries") {
        return Py::Dict(Dict(m));
    }
    let mut out: IndexMap<String, Py> = IndexMap::new();
    let mut group = None;
    for (k, v) in m.iter() {
        if k == "side_entries" {
            group = Some(v.clone());
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    if let Some(Py::Dict(g)) = group {
        for (k, v) in g.0.iter() {
            if !out.contains_key(k) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Py::Dict(Dict(out))
}

/// schemas.py:518 `SupervisorOutcome._entries_only` (ORCH-582).
fn entries_only(data: Py) -> Result<Py, Raised> {
    let mut data = ungrouped(data);
    let fire = is_str(get(&data, "kind"), "task")
        && matches!(get(&data, "task"), None | Some(Py::None))
        && matches!(get(&data, "control"), None | Some(Py::None))
        && SIDE_ENTRIES
            .iter()
            .chain(["schedule"].iter())
            .any(|k| get(&data, k).is_some_and(truthy));
    if fire {
        if let Some(m) = map_of(&mut data) {
            m.insert("kind".into(), Py::Str("reply".into()));
        }
    }
    Ok(data)
}

/// schemas.py:498 `SupervisorOutcome._one_schedule`.
fn one_schedule(data: Py) -> Result<Py, Raised> {
    let mut data = ungrouped(data);
    if let Some(m) = map_of(&mut data) {
        if let Some(one) = m.shift_remove("schedule") {
            if !matches!(one, Py::None) {
                let mut all = match m.get("schedules") {
                    Some(v) if truthy(v) => star(v)?,
                    _ => Vec::new(),
                };
                all.push(one);
                m.insert("schedules".into(), Py::List(List(all)));
            }
        }
    }
    if let Some(m) = map_of(&mut data) {
        if m.contains_key("show") || m.contains_key("about_tasks") {
            let shown = m.get("show").cloned();
            m.shift_remove("show");
            m.shift_remove("about_tasks");
            if !m.contains_key("show_schedules") {
                let listed = matches!(&shown, Some(Py::List(l)) if l.0.iter().any(|x| matches!(x, Py::Str(s) if s == "schedules")));
                m.insert("show_schedules".into(), Py::Bool(listed));
            }
        }
    }
    Ok(data)
}

/// schemas.py:540 `SupervisorOutcome.complete_action_plan`.
fn complete_action_plan(m: &mut IndexMap<String, Py>) -> Result<(), String> {
    if !is_str(m.get("kind"), "task") {
        return Ok(());
    }
    let ok = match m.get("task") {
        Some(Py::Dict(task)) => match task.0.get("success_conditions") {
            Some(Py::List(conds)) => {
                !conds.0.is_empty()
                    && conds.0.iter().all(|c| matches!(c.get("text"), Some(Py::Str(t)) if !crate::py::strip(t).is_empty()))
            }
            _ => false,
        },
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err("A new task needs nonempty success_conditions describing the requested outcome".into())
    }
}

/// conversations.py:25 `ConversationScope.concrete`.
fn concrete(m: &mut IndexMap<String, Py>) -> Result<(), String> {
    let blank = |v: Option<&Py>| matches!(v, Some(Py::Str(s)) if crate::py::strip(s).is_empty());
    let fields = ["account", "site", "app", "thread", "topic", "handover_when"];
    let recipients =
        matches!(m.get("recipients"), Some(Py::List(l)) if l.0.iter().any(|r| blank(Some(r))));
    if fields.iter().any(|f| blank(m.get(*f))) || recipients {
        return Err("Conversation scope needs concrete nonempty identifiers".into());
    }
    Ok(())
}

fn clean_field(m: &mut IndexMap<String, Py>, key: &str) {
    if let Some(Py::Str(s)) = m.get_mut(key) {
        *s = crate::answer_marks::without_stray_closing_tags(s);
    }
}

/// schemas.py:649 `WorkerReport._no_stray_tags` (ORCH-604).
fn worker_tags(m: &mut IndexMap<String, Py>) -> Result<(), String> {
    clean_field(m, "answer");
    clean_field(m, "reason");
    clean_field(m, "question"); // a non-empty question; "" and None stay as they are either way
    Ok(())
}

/// schemas.py:705 `CheckerOutcome._no_stray_tags`.
fn checker_tags(m: &mut IndexMap<String, Py>) -> Result<(), String> {
    clean_field(m, "reason");
    clean_field(m, "question");
    Ok(())
}

/// schemas.py:712 `CheckerOutcome._older_shape` (before ORCH-545).
fn older_shape(data: Py) -> Result<Py, Raised> {
    let mut m = match take_map(data) {
        Ok(m) => m,
        Err(other) => return Ok(other),
    };
    let fix = is_str(m.get("next"), "fix_answer");
    if !m.contains_key("answer_edits") && !fix {
        return Ok(Py::Dict(Dict(m)));
    }
    if fix {
        m.insert("next".into(), Py::Str("accept".into()));
    }
    let mut findings = list_or_empty(m.get("findings"))?;
    let mut dropped = list_or_empty(m.get("dropped_edits"))?;
    let edits = list_or_empty(m.shift_remove("answer_edits").as_ref())?;
    for edit in edits {
        let edit = match edit {
            Py::Dict(d) => d,
            other if !truthy(&other) => Dict(IndexMap::new()),
            other => {
                return Err(Raised {
                    exception: "AttributeError",
                    message: format!(
                        "'{}' object has no attribute 'get'",
                        crate::pydantic::type_name(&other)
                    ),
                })
            }
        };
        let text = |k: &str| match edit.0.get(k) {
            Some(v) if truthy(v) => ferb_context::pyvalue::to_str(v),
            _ => String::new(),
        };
        let (find, replace) = (text("find"), text("replace"));
        if !crate::py::strip(&find).is_empty()
            && crate::answer_marks::removes_or_marks(&find, &replace)
        {
            let kind = if replace.is_empty() {
                "contradicted"
            } else {
                "unsupported"
            };
            findings.push(dict(vec![
                ("quote", Py::Str(find)),
                ("kind", Py::Str(kind.into())),
                ("evidence", Py::Str("an earlier checker's edit".into())),
            ]));
        } else if !crate::py::strip(&find).is_empty() {
            dropped.push(Py::Str(find));
        }
    }
    m.insert("findings".into(), Py::List(List(findings)));
    m.insert("dropped_edits".into(), Py::List(List(dropped)));
    Ok(Py::Dict(Dict(m)))
}

// ---------------------------------------------------------------------------------------------------------------
// From the validated value to the structs, and back
// ---------------------------------------------------------------------------------------------------------------

fn dict(entries: Vec<(&str, Py)>) -> Py {
    Py::Dict(Dict(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    ))
}

struct R<'a>(&'a IndexMap<String, Py>);

impl R<'_> {
    fn of(v: &Py) -> R<'_> {
        match v {
            Py::Dict(d) => R(&d.0),
            _ => panic!("a validated model is a dict"),
        }
    }
    fn v(&self, k: &str) -> &Py {
        &self.0[k]
    }
    fn s(&self, k: &str) -> String {
        match self.v(k) {
            Py::Str(s) => s.clone(),
            _ => String::new(),
        }
    }
    fn os(&self, k: &str) -> Option<String> {
        match self.v(k) {
            Py::Str(s) => Some(s.clone()),
            _ => None,
        }
    }
    fn ls(&self, k: &str) -> Vec<String> {
        self.list(k)
            .iter()
            .map(|x| {
                if let Py::Str(s) = x {
                    s.clone()
                } else {
                    String::new()
                }
            })
            .collect()
    }
    fn b(&self, k: &str) -> bool {
        matches!(self.v(k), Py::Bool(true))
    }
    fn ob(&self, k: &str) -> Option<bool> {
        match self.v(k) {
            Py::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn i(&self, k: &str) -> PyInt {
        self.oi(k).unwrap_or_default()
    }
    fn oi(&self, k: &str) -> Option<PyInt> {
        match self.v(k) {
            Py::Int(i) => Some(PyInt(i.clone())),
            _ => None,
        }
    }
    fn of_(&self, k: &str) -> Option<f64> {
        match self.v(k) {
            Py::Float(f) => Some(*f),
            _ => None,
        }
    }
    fn list(&self, k: &str) -> &[Py] {
        match self.v(k) {
            Py::List(l) => &l.0,
            _ => &[],
        }
    }
    fn each<M>(&self, k: &str, f: fn(&Py) -> M) -> Vec<M> {
        self.list(k).iter().map(f).collect()
    }
    fn maybe<M>(&self, k: &str, f: fn(&Py) -> M) -> Option<M> {
        match self.v(k) {
            Py::None => None,
            v => Some(f(v)),
        }
    }
}

fn ps(s: &str) -> Py {
    Py::Str(s.to_string())
}
fn pos(s: &Option<String>) -> Py {
    s.as_deref().map_or(Py::None, ps)
}
fn pls(items: &[String]) -> Py {
    Py::List(List(items.iter().map(|s| ps(s)).collect()))
}
fn pl<M>(items: &[M], f: fn(&M) -> Py) -> Py {
    Py::List(List(items.iter().map(f).collect()))
}
fn pi(i: &PyInt) -> Py {
    Py::Int(i.0.clone())
}
fn poi(i: &Option<PyInt>) -> Py {
    i.as_ref().map_or(Py::None, pi)
}
fn pof(f: &Option<f64>) -> Py {
    f.map_or(Py::None, Py::Float)
}
fn pob(b: &Option<bool>) -> Py {
    b.map_or(Py::None, Py::Bool)
}
fn pom<M>(m: &Option<M>, f: fn(&M) -> Py) -> Py {
    m.as_ref().map_or(Py::None, f)
}

macro_rules! api {
    ($name:literal, $ty:ty, $validate:ident, $validate_json:ident, $dump:ident, $dump_json:ident, $from:ident, $to:ident) => {
        #[doc = concat!("`", $name, ".model_validate(value)`.")]
        pub fn $validate(v: &Py) -> Result<$ty, ValidationError> {
            validate_model($name, v, false).map(|x| $from(&x))
        }
        #[doc = concat!("`", $name, ".model_validate_json(text)`.")]
        pub fn $validate_json(text: &str) -> Result<$ty, ValidationError> {
            validate_model_json($name, text).map(|x| $from(&x))
        }
        #[doc = concat!("`", $name, "(...).model_dump()`.")]
        pub fn $dump(m: &$ty) -> Py {
            $to(m)
        }
        #[doc = concat!("`", $name, "(...).model_dump_json()`.")]
        pub fn $dump_json(m: &$ty) -> String {
            dump_json(&$to(m))
        }
    };
}

api!(
    "When",
    When,
    when_validate,
    when_validate_json,
    dump_when,
    dump_json_when,
    when_from,
    when_to
);
api!(
    "Probe",
    Probe,
    probe_validate,
    probe_validate_json,
    dump_probe,
    dump_json_probe,
    probe_from,
    probe_to
);
api!(
    "ConversationScope",
    ConversationScope,
    conversation_scope_validate,
    conversation_scope_validate_json,
    dump_conversation_scope,
    dump_json_conversation_scope,
    conv_from,
    conv_to
);
api!(
    "Step",
    Step,
    step_validate,
    step_validate_json,
    dump_step,
    dump_json_step,
    step_from,
    step_to
);
api!(
    "SendScope",
    SendScope,
    send_scope_validate,
    send_scope_validate_json,
    dump_send_scope,
    dump_json_send_scope,
    scope_from,
    scope_to
);
api!(
    "Part",
    Part,
    part_validate,
    part_validate_json,
    dump_part,
    dump_json_part,
    part_from,
    part_to
);
api!(
    "Plan",
    Plan,
    plan_validate,
    plan_validate_json,
    dump_plan,
    dump_json_plan,
    plan_from,
    plan_to
);
api!(
    "TaskSpec",
    TaskSpec,
    task_spec_validate,
    task_spec_validate_json,
    dump_task_spec,
    dump_json_task_spec,
    task_from,
    task_to
);
api!(
    "SupervisorOutcome",
    SupervisorOutcome,
    supervisor_outcome_validate,
    supervisor_outcome_validate_json,
    dump_supervisor_outcome,
    dump_json_supervisor_outcome,
    sup_from,
    sup_to
);
api!(
    "WorkerReport",
    WorkerReport,
    worker_report_validate,
    worker_report_validate_json,
    dump_worker_report,
    dump_json_worker_report,
    worker_from,
    worker_to
);
api!(
    "CheckerOutcome",
    CheckerOutcome,
    checker_outcome_validate,
    checker_outcome_validate_json,
    dump_checker_outcome,
    dump_json_checker_outcome,
    checker_from,
    checker_to
);
api!(
    "Finding",
    Finding,
    finding_validate,
    finding_validate_json,
    dump_finding,
    dump_json_finding,
    finding_from,
    finding_to
);
api!(
    "FixOutcome",
    FixOutcome,
    fix_outcome_validate,
    fix_outcome_validate_json,
    dump_fix_outcome,
    dump_json_fix_outcome,
    fix_from,
    fix_to
);
api!(
    "RecheckOutcome",
    RecheckOutcome,
    recheck_outcome_validate,
    recheck_outcome_validate_json,
    dump_recheck_outcome,
    dump_json_recheck_outcome,
    recheck_from,
    recheck_to
);
api!(
    "SummaryOutcome",
    SummaryOutcome,
    summary_outcome_validate,
    summary_outcome_validate_json,
    dump_summary_outcome,
    dump_json_summary_outcome,
    summary_from,
    summary_to
);
api!(
    "QuestionOutcome",
    QuestionOutcome,
    question_outcome_validate,
    question_outcome_validate_json,
    dump_question_outcome,
    dump_json_question_outcome,
    question_from,
    question_to
);
api!(
    "ScheduleRequest",
    ScheduleRequest,
    schedule_request_validate,
    schedule_request_validate_json,
    dump_schedule_request,
    dump_json_schedule_request,
    schedule_from,
    schedule_to
);
api!(
    "TimerRequest",
    TimerRequest,
    timer_request_validate,
    timer_request_validate_json,
    dump_timer_request,
    dump_json_timer_request,
    timer_from,
    timer_to
);
api!(
    "Remember",
    Remember,
    remember_validate,
    remember_validate_json,
    dump_remember,
    dump_json_remember,
    remember_from,
    remember_to
);
api!(
    "ScheduleChange",
    ScheduleChange,
    schedule_change_validate,
    schedule_change_validate_json,
    dump_schedule_change,
    dump_json_schedule_change,
    change_from,
    change_to
);

/// `TaskSpec(...).model_dump_json(exclude={"plan"})` (steering.py:263).
pub fn dump_json_task_spec_without_plan(m: &TaskSpec) -> String {
    let mut v = task_to(m);
    if let Py::Dict(d) = &mut v {
        d.0.shift_remove("plan");
    }
    dump_json(&v)
}

/// Validate any of the models by its class name (`model_validate` or, with `json`, `model_validate_json`) and give
/// its `model_dump()`: what the fixture replays.
pub fn validate_by_name(name: &str, input: &Py, text: Option<&str>) -> Result<Py, ValidationError> {
    let name: &'static str = models()
        .into_iter()
        .map(|m| m.name)
        .find(|n| *n == name)
        .unwrap_or_else(|| panic!("no model {name}"));
    let raw = match text {
        Some(t) => validate_model_json(name, t)?,
        None => validate_model(name, input, false)?,
    };
    // through the struct, so the dump the fixture checks is the struct's
    Ok(match name {
        "When" => when_to(&when_from(&raw)),
        "Probe" => probe_to(&probe_from(&raw)),
        "ConversationScope" => conv_to(&conv_from(&raw)),
        "Step" => step_to(&step_from(&raw)),
        "Plan" => plan_to(&plan_from(&raw)),
        "TaskSpec" => task_to(&task_from(&raw)),
        "SupervisorOutcome" => sup_to(&sup_from(&raw)),
        "WorkerReport" => worker_to(&worker_from(&raw)),
        "CheckerOutcome" => checker_to(&checker_from(&raw)),
        "FixOutcome" => fix_to(&fix_from(&raw)),
        "RecheckOutcome" => recheck_to(&recheck_from(&raw)),
        "SummaryOutcome" => summary_to(&summary_from(&raw)),
        "QuestionOutcome" => question_to(&question_from(&raw)),
        _ => raw,
    })
}

/// `model_dump_json()` of a `model_dump()` value.
pub fn dump_json_value(v: &Py) -> String {
    dump_json(v)
}

fn when_from(v: &Py) -> When {
    let r = R::of(v);
    When {
        in_minutes: r.of_("in_minutes"),
        in_days: r.oi("in_days"),
        day: r.os("day"),
        date: r.os("date"),
        time: r.os("time"),
    }
}
fn when_to(m: &When) -> Py {
    dict(vec![
        ("in_minutes", pof(&m.in_minutes)),
        ("in_days", poi(&m.in_days)),
        ("day", pos(&m.day)),
        ("date", pos(&m.date)),
        ("time", pos(&m.time)),
    ])
}

fn probe_from(v: &Py) -> Probe {
    let r = R::of(v);
    Probe {
        tool: r.s("tool"),
        args_json: r.s("args_json"),
        field: r.s("field"),
        op: r.s("op"),
        value_json: r.s("value_json"),
    }
}
fn probe_to(m: &Probe) -> Py {
    dict(vec![
        ("tool", ps(&m.tool)),
        ("args_json", ps(&m.args_json)),
        ("field", ps(&m.field)),
        ("op", ps(&m.op)),
        ("value_json", ps(&m.value_json)),
    ])
}

fn conv_from(v: &Py) -> ConversationScope {
    let r = R::of(v);
    ConversationScope {
        identity: r.s("identity"),
        account: r.s("account"),
        site: r.s("site"),
        app: r.s("app"),
        thread: r.s("thread"),
        recipients: r.ls("recipients"),
        topic: r.s("topic"),
        handover_when: r.s("handover_when"),
        window_id: r.os("window_id"),
    }
}
fn conv_to(m: &ConversationScope) -> Py {
    dict(vec![
        ("identity", ps(&m.identity)),
        ("account", ps(&m.account)),
        ("site", ps(&m.site)),
        ("app", ps(&m.app)),
        ("thread", ps(&m.thread)),
        ("recipients", pls(&m.recipients)),
        ("topic", ps(&m.topic)),
        ("handover_when", ps(&m.handover_when)),
        ("window_id", pos(&m.window_id)),
    ])
}

fn match_from(v: &Py) -> EventMatch {
    let r = R::of(v);
    EventMatch {
        field: r.s("field"),
        value: r.s("value"),
    }
}
fn match_to(m: &EventMatch) -> Py {
    dict(vec![("field", ps(&m.field)), ("value", ps(&m.value))])
}

fn repeat_from(v: &Py) -> Repeat {
    let r = R::of(v);
    Repeat {
        end_when: r.s("end_when"),
        event: r.os("event"),
        event_match: r.each("event_match", match_from),
    }
}
fn repeat_to(m: &Repeat) -> Py {
    dict(vec![
        ("end_when", ps(&m.end_when)),
        ("event", pos(&m.event)),
        ("event_match", pl(&m.event_match, match_to)),
    ])
}

fn step_from(v: &Py) -> Step {
    let r = R::of(v);
    Step {
        id: r.s("id"),
        kind: r.s("kind"),
        label: r.s("label"),
        goal: r.s("goal"),
        depends_on: r.ls("depends_on"),
        inputs: r.ls("inputs"),
        worker_type: r.s("worker_type"),
        conditions: r.ls("conditions"),
        until: r.os("until"),
        event: r.os("event"),
        event_match: r.each("event_match", match_from),
        conversation: r.maybe("conversation", conv_from),
        question: r.s("question"),
        options: r.ls("options"),
        repeat: r.maybe("repeat", repeat_from),
        earlier_results: r.ls("earlier_results"),
    }
}
fn step_to(m: &Step) -> Py {
    dict(vec![
        ("id", ps(&m.id)),
        ("kind", ps(&m.kind)),
        ("label", ps(&m.label)),
        ("goal", ps(&m.goal)),
        ("depends_on", pls(&m.depends_on)),
        ("inputs", pls(&m.inputs)),
        ("worker_type", ps(&m.worker_type)),
        ("conditions", pls(&m.conditions)),
        ("until", pos(&m.until)),
        ("event", pos(&m.event)),
        ("event_match", pl(&m.event_match, match_to)),
        ("conversation", pom(&m.conversation, conv_to)),
        ("question", ps(&m.question)),
        ("options", pls(&m.options)),
        ("repeat", pom(&m.repeat, repeat_to)),
        ("earlier_results", pls(&m.earlier_results)),
    ])
}

fn scope_from(v: &Py) -> SendScope {
    let r = R::of(v);
    SendScope {
        capability: r.s("capability"),
        identity: r.s("identity"),
        account: r.os("account"),
        site: r.os("site"),
        recipients: r.ls("recipients"),
        content: r.os("content"),
    }
}
fn scope_to(m: &SendScope) -> Py {
    dict(vec![
        ("capability", ps(&m.capability)),
        ("identity", ps(&m.identity)),
        ("account", pos(&m.account)),
        ("site", pos(&m.site)),
        ("recipients", pls(&m.recipients)),
        ("content", pos(&m.content)),
    ])
}

fn part_from(v: &Py) -> Part {
    let r = R::of(v);
    Part {
        id: r.s("id"),
        quote: r.s("quote"),
        label: r.s("label"),
        direct: r.os("direct"),
        answer_step: r.os("answer_step"),
        conditions: r
            .list("conditions")
            .iter()
            .map(|x| {
                if let Py::Int(i) = x {
                    PyInt(i.clone())
                } else {
                    PyInt::default()
                }
            })
            .collect(),
    }
}
fn part_to(m: &Part) -> Py {
    dict(vec![
        ("id", ps(&m.id)),
        ("quote", ps(&m.quote)),
        ("label", ps(&m.label)),
        ("direct", pos(&m.direct)),
        ("answer_step", pos(&m.answer_step)),
        ("conditions", pl(&m.conditions, pi)),
    ])
}

fn plan_from(v: &Py) -> Plan {
    let r = R::of(v);
    Plan {
        steps: r.each("steps", step_from),
        input_paths: r.ls("input_paths"),
        big_job: r.b("big_job"),
        time_estimate: r.s("time_estimate"),
        cost_estimate: r.s("cost_estimate"),
        standing_sends: r.each("standing_sends", scope_from),
    }
}
fn plan_to(m: &Plan) -> Py {
    dict(vec![
        ("steps", pl(&m.steps, step_to)),
        ("input_paths", pls(&m.input_paths)),
        ("big_job", Py::Bool(m.big_job)),
        ("time_estimate", ps(&m.time_estimate)),
        ("cost_estimate", ps(&m.cost_estimate)),
        ("standing_sends", pl(&m.standing_sends, scope_to)),
    ])
}

fn targets_from(v: &Py) -> TargetsSpec {
    let r = R::of(v);
    TargetsSpec {
        apps: r.ls("apps"),
        sites: r.ls("sites"),
        paths: r.ls("paths"),
        recipients: r.ls("recipients"),
    }
}
fn targets_to(m: &TargetsSpec) -> Py {
    dict(vec![
        ("apps", pls(&m.apps)),
        ("sites", pls(&m.sites)),
        ("paths", pls(&m.paths)),
        ("recipients", pls(&m.recipients)),
    ])
}

fn check_from(v: &Py) -> Check {
    let r = R::of(v);
    Check {
        kind: r.s("kind"),
        path: r.os("path"),
        text: r.os("text"),
        url_contains: r.os("url_contains"),
        site: r.os("site"),
        expected: r.os("expected"),
        tool: r.os("tool"),
    }
}
fn check_to(m: &Check) -> Py {
    dict(vec![
        ("kind", ps(&m.kind)),
        ("path", pos(&m.path)),
        ("text", pos(&m.text)),
        ("url_contains", pos(&m.url_contains)),
        ("site", pos(&m.site)),
        ("expected", pos(&m.expected)),
        ("tool", pos(&m.tool)),
    ])
}

fn condition_from(v: &Py) -> Condition {
    let r = R::of(v);
    Condition {
        text: r.s("text"),
        owner_text: r.s("owner_text"),
        check: check_from(r.v("check")),
        layer: r.s("layer"),
    }
}
fn condition_to(m: &Condition) -> Py {
    dict(vec![
        ("text", ps(&m.text)),
        ("owner_text", ps(&m.owner_text)),
        ("check", check_to(&m.check)),
        ("layer", ps(&m.layer)),
    ])
}

fn task_from(v: &Py) -> TaskSpec {
    let r = R::of(v);
    TaskSpec {
        goal: r.s("goal"),
        name: r.s("name"),
        identity: r.s("identity"),
        worker_type: r.s("worker_type"),
        targets: targets_from(r.v("targets")),
        success_conditions: r.each("success_conditions", condition_from),
        plan: r.maybe("plan", plan_from),
        folders: r.ls("folders"),
        keep_tabs_open: r.b("keep_tabs_open"),
        parts: r.each("parts", part_from),
        earlier_results: r.ls("earlier_results"),
    }
}
fn task_to(m: &TaskSpec) -> Py {
    dict(vec![
        ("goal", ps(&m.goal)),
        ("name", ps(&m.name)),
        ("identity", ps(&m.identity)),
        ("worker_type", ps(&m.worker_type)),
        ("targets", targets_to(&m.targets)),
        (
            "success_conditions",
            pl(&m.success_conditions, condition_to),
        ),
        ("plan", pom(&m.plan, plan_to)),
        ("folders", pls(&m.folders)),
        ("keep_tabs_open", Py::Bool(m.keep_tabs_open)),
        ("parts", pl(&m.parts, part_to)),
        ("earlier_results", pls(&m.earlier_results)),
    ])
}

fn until_from(v: &Py) -> Until {
    let r = R::of(v);
    Until {
        quote: r.s("quote"),
        day: r.os("day"),
        date: r.os("date"),
        in_days: r.oi("in_days"),
    }
}
fn until_to(m: &Until) -> Py {
    dict(vec![
        ("quote", ps(&m.quote)),
        ("day", pos(&m.day)),
        ("date", pos(&m.date)),
        ("in_days", poi(&m.in_days)),
    ])
}

fn s_until_from(v: &Py) -> ScheduleUntil {
    let r = R::of(v);
    ScheduleUntil {
        quote: r.s("quote"),
        day: r.os("day"),
        date: r.os("date"),
        in_days: r.oi("in_days"),
        time: r.os("time"),
        in_minutes: r.of_("in_minutes"),
    }
}
fn s_until_to(m: &ScheduleUntil) -> Py {
    dict(vec![
        ("quote", ps(&m.quote)),
        ("day", pos(&m.day)),
        ("date", pos(&m.date)),
        ("in_days", poi(&m.in_days)),
        ("time", pos(&m.time)),
        ("in_minutes", pof(&m.in_minutes)),
    ])
}

fn remember_from(v: &Py) -> Remember {
    let r = R::of(v);
    Remember {
        kind: r.s("kind"),
        subject: r.s("subject"),
        key: r.s("key"),
        content: r.s("content"),
        quote: r.s("quote"),
        login_origin: r.os("login_origin"),
        until: r.maybe("until", until_from),
        replaces: r.ls("replaces"),
        review_in_days: r.i("review_in_days"),
    }
}
fn remember_to(m: &Remember) -> Py {
    dict(vec![
        ("kind", ps(&m.kind)),
        ("subject", ps(&m.subject)),
        ("key", ps(&m.key)),
        ("content", ps(&m.content)),
        ("quote", ps(&m.quote)),
        ("login_origin", pos(&m.login_origin)),
        ("until", pom(&m.until, until_to)),
        ("replaces", pls(&m.replaces)),
        ("review_in_days", pi(&m.review_in_days)),
    ])
}

fn forget_from(v: &Py) -> ForgetRequest {
    let r = R::of(v);
    ForgetRequest {
        item_id: r.s("item_id"),
        quote: r.s("quote"),
    }
}
fn forget_to(m: &ForgetRequest) -> Py {
    dict(vec![("item_id", ps(&m.item_id)), ("quote", ps(&m.quote))])
}

fn answer_from(v: &Py) -> AnswerQuestion {
    let r = R::of(v);
    AnswerQuestion {
        approval_id: r.s("approval_id"),
        answer: r.s("answer"),
    }
}
fn answer_to(m: &AnswerQuestion) -> Py {
    dict(vec![
        ("approval_id", ps(&m.approval_id)),
        ("answer", ps(&m.answer)),
    ])
}

fn control_from(v: &Py) -> TaskControl {
    let r = R::of(v);
    TaskControl {
        task_id: r.s("task_id"),
        action: r.s("action"),
        new_goal: r.b("new_goal"),
        standing_sends: match r.v("standing_sends") {
            Py::Str(_) => StandingSends::Unchanged,
            _ => StandingSends::Scopes(r.each("standing_sends", scope_from)),
        },
        supersedes: r.ls("supersedes"),
    }
}
fn control_to(m: &TaskControl) -> Py {
    dict(vec![
        ("task_id", ps(&m.task_id)),
        ("action", ps(&m.action)),
        ("new_goal", Py::Bool(m.new_goal)),
        (
            "standing_sends",
            match &m.standing_sends {
                StandingSends::Unchanged => ps("unchanged"),
                StandingSends::Scopes(s) => pl(s, scope_to),
            },
        ),
        ("supersedes", pls(&m.supersedes)),
    ])
}

fn random_from(v: &Py) -> RandomTimes {
    let r = R::of(v);
    RandomTimes {
        count: r.i("count"),
        start: r.s("start"),
        end: r.s("end"),
    }
}
fn random_to(m: &RandomTimes) -> Py {
    dict(vec![
        ("count", pi(&m.count)),
        ("start", ps(&m.start)),
        ("end", ps(&m.end)),
    ])
}

fn timing_from(v: &Py) -> ScheduleTiming {
    let r = R::of(v);
    ScheduleTiming {
        repeat: r.s("repeat"),
        times: r.ls("times"),
        every_minutes: r.i("every_minutes"),
        weekday: r.s("weekday"),
        random: r.each("random", random_from),
    }
}
fn timing_to(m: &ScheduleTiming) -> Py {
    dict(vec![
        ("repeat", ps(&m.repeat)),
        ("times", pls(&m.times)),
        ("every_minutes", pi(&m.every_minutes)),
        ("weekday", ps(&m.weekday)),
        ("random", pl(&m.random, random_to)),
    ])
}

fn schedule_from(v: &Py) -> ScheduleRequest {
    let r = R::of(v);
    ScheduleRequest {
        request: r.s("request"),
        name: r.s("name"),
        when: when_from(r.v("when")),
        repeat: r.s("repeat"),
        weekday: r.os("weekday"),
        every_minutes: r.oi("every_minutes"),
        times: r.ls("times"),
        random: r.each("random", random_from),
        quote: r.s("quote"),
        targets: targets_from(r.v("targets")),
        standing_sends: r.each("standing_sends", scope_from),
        watch: r.s("watch"),
        until: r.maybe("until", s_until_from),
        when_active: r.b("when_active"),
        folders: r.ls("folders"),
        identity: r.os("identity"),
        worker_type: r.s("worker_type"),
        profile: r.s("profile"),
        account: r.s("account"),
    }
}
fn schedule_to(m: &ScheduleRequest) -> Py {
    dict(vec![
        ("request", ps(&m.request)),
        ("name", ps(&m.name)),
        ("when", when_to(&m.when)),
        ("repeat", ps(&m.repeat)),
        ("weekday", pos(&m.weekday)),
        ("every_minutes", poi(&m.every_minutes)),
        ("times", pls(&m.times)),
        ("random", pl(&m.random, random_to)),
        ("quote", ps(&m.quote)),
        ("targets", targets_to(&m.targets)),
        ("standing_sends", pl(&m.standing_sends, scope_to)),
        ("watch", ps(&m.watch)),
        ("until", pom(&m.until, s_until_to)),
        ("when_active", Py::Bool(m.when_active)),
        ("folders", pls(&m.folders)),
        ("identity", pos(&m.identity)),
        ("worker_type", ps(&m.worker_type)),
        ("profile", ps(&m.profile)),
        ("account", ps(&m.account)),
    ])
}

fn change_from(v: &Py) -> ScheduleChange {
    let r = R::of(v);
    ScheduleChange {
        id: r.s("id"),
        action: r.s("action"),
        quote: r.s("quote"),
        request: r.s("request"),
        timing: timing_from(r.v("timing")),
        until: r.maybe("until", s_until_from),
        name: r.s("name"),
    }
}
fn change_to(m: &ScheduleChange) -> Py {
    dict(vec![
        ("id", ps(&m.id)),
        ("action", ps(&m.action)),
        ("quote", ps(&m.quote)),
        ("request", ps(&m.request)),
        ("timing", timing_to(&m.timing)),
        ("until", pom(&m.until, s_until_to)),
        ("name", ps(&m.name)),
    ])
}

fn cancel_from(v: &Py) -> ReminderCancel {
    let r = R::of(v);
    ReminderCancel {
        id: r.s("id"),
        quote: r.s("quote"),
    }
}
fn cancel_to(m: &ReminderCancel) -> Py {
    dict(vec![("id", ps(&m.id)), ("quote", ps(&m.quote))])
}

fn timer_from(v: &Py) -> TimerRequest {
    let r = R::of(v);
    TimerRequest {
        when: when_from(r.v("when")),
        message: r.s("message"),
        quote: r.s("quote"),
        when_active: r.b("when_active"),
        name: r.s("name"),
        kind: r.s("kind"),
    }
}
fn timer_to(m: &TimerRequest) -> Py {
    dict(vec![
        ("when", when_to(&m.when)),
        ("message", ps(&m.message)),
        ("quote", ps(&m.quote)),
        ("when_active", Py::Bool(m.when_active)),
        ("name", ps(&m.name)),
        ("kind", ps(&m.kind)),
    ])
}

fn conv_change_from(v: &Py) -> ConversationChange {
    let r = R::of(v);
    ConversationChange {
        id: r.s("id"),
        action: r.s("action"),
        quote: r.s("quote"),
    }
}
fn conv_change_to(m: &ConversationChange) -> Py {
    dict(vec![
        ("id", ps(&m.id)),
        ("action", ps(&m.action)),
        ("quote", ps(&m.quote)),
    ])
}

fn sup_from(v: &Py) -> SupervisorOutcome {
    let r = R::of(v);
    SupervisorOutcome {
        control: r.maybe("control", control_from),
        kind: r.s("kind"),
        text: r.s("text"),
        answers_question: r.b("answers_question"),
        answer_question: r.maybe("answer_question", answer_from),
        task: r.maybe("task", task_from),
        remember: r.each("remember", remember_from),
        forget: r.each("forget", forget_from),
        timers: r.each("timers", timer_from),
        schedules: r.each("schedules", schedule_from),
        schedule_changes: r.each("schedule_changes", change_from),
        show_schedules: r.b("show_schedules"),
        reminder_cancels: r.each("reminder_cancels", cancel_from),
        conversation_changes: r.each("conversation_changes", conv_change_from),
    }
}
fn sup_to(m: &SupervisorOutcome) -> Py {
    dict(vec![
        ("control", pom(&m.control, control_to)),
        ("kind", ps(&m.kind)),
        ("text", ps(&m.text)),
        ("answers_question", Py::Bool(m.answers_question)),
        ("answer_question", pom(&m.answer_question, answer_to)),
        ("task", pom(&m.task, task_to)),
        ("remember", pl(&m.remember, remember_to)),
        ("forget", pl(&m.forget, forget_to)),
        ("timers", pl(&m.timers, timer_to)),
        ("schedules", pl(&m.schedules, schedule_to)),
        ("schedule_changes", pl(&m.schedule_changes, change_to)),
        ("show_schedules", Py::Bool(m.show_schedules)),
        ("reminder_cancels", pl(&m.reminder_cancels, cancel_to)),
        (
            "conversation_changes",
            pl(&m.conversation_changes, conv_change_to),
        ),
    ])
}

fn worker_from(v: &Py) -> WorkerReport {
    let r = R::of(v);
    WorkerReport {
        status: r.s("status"),
        reason: r.s("reason"),
        question: r.os("question"),
        answer: r.s("answer"),
        artifacts: r.ls("artifacts"),
        how: r.s("how"),
        input_notes: r.list("input_notes").to_vec(),
        conversation_action: r.os("conversation_action"),
        conversation_proposals: r.list("conversation_proposals").to_vec(),
        watch_met: r.ob("watch_met"),
        loop_action: r.os("loop_action"),
        probe: r.maybe("probe", probe_from),
        part_updates: r.list("part_updates").to_vec(),
    }
}
fn worker_to(m: &WorkerReport) -> Py {
    dict(vec![
        ("status", ps(&m.status)),
        ("reason", ps(&m.reason)),
        ("question", pos(&m.question)),
        ("answer", ps(&m.answer)),
        ("artifacts", pls(&m.artifacts)),
        ("how", ps(&m.how)),
        ("input_notes", Py::List(List(m.input_notes.clone()))),
        ("conversation_action", pos(&m.conversation_action)),
        (
            "conversation_proposals",
            Py::List(List(m.conversation_proposals.clone())),
        ),
        ("watch_met", pob(&m.watch_met)),
        ("loop_action", pos(&m.loop_action)),
        ("probe", pom(&m.probe, probe_to)),
        ("part_updates", Py::List(List(m.part_updates.clone()))),
    ])
}

fn question_from(v: &Py) -> QuestionOutcome {
    let r = R::of(v);
    QuestionOutcome {
        ask_owner: r.b("ask_owner"),
        answer: r.s("answer"),
        question: r.s("question"),
    }
}
fn question_to(m: &QuestionOutcome) -> Py {
    dict(vec![
        ("ask_owner", Py::Bool(m.ask_owner)),
        ("answer", ps(&m.answer)),
        ("question", ps(&m.question)),
    ])
}

fn result_from(v: &Py) -> CheckerResult {
    let r = R::of(v);
    CheckerResult {
        index: r.i("index"),
        met: r.ob("met"),
        applies: r.b("applies"),
        evidence: r.s("evidence"),
        how: r.os("how"),
    }
}
fn result_to(m: &CheckerResult) -> Py {
    dict(vec![
        ("index", pi(&m.index)),
        ("met", pob(&m.met)),
        ("applies", Py::Bool(m.applies)),
        ("evidence", ps(&m.evidence)),
        ("how", pos(&m.how)),
    ])
}

fn finding_from(v: &Py) -> Finding {
    let r = R::of(v);
    Finding {
        quote: r.s("quote"),
        kind: r.s("kind"),
        evidence: r.s("evidence"),
    }
}
fn finding_to(m: &Finding) -> Py {
    dict(vec![
        ("quote", ps(&m.quote)),
        ("kind", ps(&m.kind)),
        ("evidence", ps(&m.evidence)),
    ])
}

fn fix_from(v: &Py) -> FixOutcome {
    FixOutcome {
        answer: R::of(v).s("answer"),
    }
}
fn fix_to(m: &FixOutcome) -> Py {
    dict(vec![("answer", ps(&m.answer))])
}

fn recheck_from(v: &Py) -> RecheckOutcome {
    RecheckOutcome {
        findings: R::of(v).each("findings", finding_from),
    }
}
fn recheck_to(m: &RecheckOutcome) -> Py {
    dict(vec![("findings", pl(&m.findings, finding_to))])
}

fn summary_from(v: &Py) -> SummaryOutcome {
    SummaryOutcome {
        summary: R::of(v).s("summary"),
    }
}
fn summary_to(m: &SummaryOutcome) -> Py {
    dict(vec![("summary", ps(&m.summary))])
}

fn checker_from(v: &Py) -> CheckerOutcome {
    let r = R::of(v);
    CheckerOutcome {
        results: r.each("results", result_from),
        next: r.s("next"),
        findings: r.each("findings", finding_from),
        missing: r.s("missing"),
        instructions: r.s("instructions"),
        worker_type: r.os("worker_type"),
        question: r.s("question"),
        reason: r.s("reason"),
        dropped_edits: r.ls("dropped_edits"),
    }
}
fn checker_to(m: &CheckerOutcome) -> Py {
    dict(vec![
        ("results", pl(&m.results, result_to)),
        ("next", ps(&m.next)),
        ("findings", pl(&m.findings, finding_to)),
        ("missing", ps(&m.missing)),
        ("instructions", ps(&m.instructions)),
        ("worker_type", pos(&m.worker_type)),
        ("question", ps(&m.question)),
        ("reason", ps(&m.reason)),
        ("dropped_edits", pls(&m.dropped_edits)),
    ])
}
