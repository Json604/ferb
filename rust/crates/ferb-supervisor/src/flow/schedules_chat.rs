//! A schedule made, edited or changed from chat (ferb/daemon/areas/schedules.py:238 `_schedule_create`, :354
//! `_schedule_problems`, :382 `_schedule_scopes`, :466 `_schedule_edit_from_chat`; Python's `SchedulePort`), behind
//! [`SchedulesChat`]: the supervisor's side entries call it, and the daemon program implements it on its schedule
//! stores (`daemon_seams::DaemonSchedules`, group A's state). The functions here run on a [`Ctx`]: FERB's timer and
//! schedule stores as the daemon keeps them (the timer store tells its changes), its journal, the timer loop's wake and
//! the few reads another part owns.
//!
//! The owner's words are quote-checked here too (ORCH-519, ORCH-520): the supervisor checks first, against this turn and
//! the chat's recent owner messages, and hands over the words the quote was found in.

use std::collections::BTreeSet;

use ferb_clock::clock::ZonedClock;
use ferb_clock::py::{self as cpy, Time};
use ferb_clock::schedule_store::Schedules;
use ferb_clock::schedules::{
    plan_edit, plan_new, said_list, scope_said, until_said, watch_said, Plan, Spec, TimingSpec,
    Window, RUN_SCOPE, TIMING,
};
use ferb_clock::timers::Timers;
use ferb_orchestrator::models::{ScheduleChange, ScheduleRequest, ScheduleTiming, SendScope};
use ferb_orchestrator::ports::BoxFuture;
use ferb_policy::small::blocked;
use ferb_sanitize::{origin, Redactor};
use serde_json::{json, Map, Value};

/// A new schedule or a schedule change the planner wrote (`_schedule_problems`' entry).
#[derive(Clone, Debug)]
pub enum Entry {
    New(Box<ScheduleRequest>),
    Change(Box<ScheduleChange>),
}

/// What the supervisor's side entries call (Python's `SchedulePort`: `on_schedule`, `on_schedule_edit`,
/// `on_schedule_change`, `on_reminder_cancel`, `schedule_problems`, `schedules_card`).
pub trait SchedulesChat: Send + Sync {
    /// `_schedule_create`: the owner's line, and the schedule's event card when one was set.
    fn create<'a>(
        &'a self,
        spec: &'a ScheduleRequest,
        own_words: &'a str,
        request_id: &'a str,
        origins: &'a BTreeSet<String>,
        request_origins: &'a BTreeSet<String>,
    ) -> BoxFuture<'a, (String, Option<Value>)>;
    /// `_schedule_edit_from_chat` (T-1066).
    fn edit<'a>(
        &'a self,
        ch: &'a ScheduleChange,
        own_words: &'a str,
        request_id: &'a str,
        origins: &'a BTreeSet<String>,
        request_origins: &'a BTreeSet<String>,
    ) -> BoxFuture<'a, (String, Option<Value>)>;
    /// `_schedule_change_from_chat` (cancel, pause, resume, set_end).
    fn change<'a>(&'a self, ch: &'a ScheduleChange, own_words: &'a str) -> BoxFuture<'a, String>;
    /// `_reminder_cancel_from_chat`.
    fn reminder_cancel<'a>(
        &'a self,
        rc: &'a ferb_orchestrator::models::ReminderCancel,
        own_words: &'a str,
    ) -> BoxFuture<'a, String>;
    /// `_schedule_problems`.
    fn problems<'a>(&'a self, entry: Entry) -> BoxFuture<'a, Vec<String>>;
    /// `_schedules_card`: the owner's schedules as FERB's records hold them (line, card); None when unreadable.
    fn card<'a>(&'a self) -> BoxFuture<'a, Option<(String, Value)>>;
}

/// `_book(s, due, return_after=)`: a timer for one of a schedule's runs, the timer loop told.
pub type Book<'b> = dyn Fn(
        &ferb_clock::schedules::Schedule,
        &Time,
        Option<&Time>,
    ) -> Result<ferb_clock::timers::Timer, String>
    + 'b;

/// What a schedule from chat is worked out on.
pub struct Ctx<'a> {
    /// FERB's timers (`self.timers`, telling `timer.changed`) and schedules.
    pub timers: &'a Timers<'a>,
    pub schedules: &'a Schedules<'a>,
    pub clock: &'a dyn ZonedClock,
    pub new_id: &'a dyn Fn() -> String,
    pub redactor: &'a Redactor,
    /// `self._journal().append(kind, data, origin=)`.
    pub journal: &'a dyn Fn(&str, Value, Option<&str>),
    /// `_book`: a timer for one of a schedule's runs (due, return after), the timer loop told.
    pub book: &'a Book<'a>,
    /// `_typed_behind_memory(request_id)`.
    pub typed_behind_memory: &'a dyn Fn(&str) -> Vec<String>,
    /// The owner's home folder (`Path.home()`) and FERB's workspace.
    pub home: &'a str,
    pub workspace: &'a str,
    /// The runtime catalog's name for a worker type (`scope_said`).
    pub worker_label: &'a dyn Fn(&str) -> String,
    /// APP-777: a run of a schedule that browses as FERB (None: nothing to ask or say).
    pub browsing: Option<&'a dyn Browsing>,
}

/// APP-777 for a schedule whose runs browse as FERB (`_identity_profile`, `_owner_browsing_profile`, `_browsing`).
pub trait Browsing {
    /// With no profile of FERB's own, the owner's profile its runs browse in, when the owner chose that (its display
    /// name); None otherwise.
    fn as_owner(&self, request_id: &str) -> Option<String>;
    /// `_browsing(True, request_id)`: ask the owner now which profile its runs use.
    fn ask(&self, request_id: &str);
}

/// The planner's `When` as ferb-clock reads it.
pub fn clock_when(w: &ferb_orchestrator::models::When) -> ferb_clock::when::When {
    ferb_clock::when::When {
        in_minutes: w.in_minutes,
        in_days: w.in_days.as_ref().and_then(|d| d.0.parse().ok()),
        day: w.day.clone(),
        date: w.date.clone(),
        time: w.time.clone(),
    }
}

fn windows(random: &[ferb_orchestrator::models::RandomTimes]) -> Vec<Window> {
    random
        .iter()
        .map(|r| Window {
            count: r.count.0.parse().unwrap_or(0),
            start: r.start.clone(),
            end: r.end.clone(),
        })
        .collect()
}

/// The planner's schedule as `plan_new` reads it.
fn spec_of(s: &ScheduleRequest) -> Spec {
    Spec {
        request: s.request.clone(),
        when: clock_when(&s.when),
        repeat: s.repeat.clone(),
        weekday: s.weekday.clone(),
        every_minutes: s.every_minutes.as_ref().and_then(|m| m.0.parse().ok()),
        times: s.times.clone(),
        random: windows(&s.random),
        when_active: s.when_active,
        until: s.until.as_ref().map(|u| clock_when(&u.when())),
    }
}

fn timing_of(t: &ScheduleTiming) -> TimingSpec {
    TimingSpec {
        repeat: t.repeat.clone(),
        times: t.times.clone(),
        every_minutes: t.every_minutes.0.parse().unwrap_or(0),
        weekday: t.weekday.clone(),
        random: windows(&t.random),
    }
}

/// `ScheduleRequest.run_scope()` (schemas.py:588): what every run uses, as the schedule records it; None when the
/// planner stated none.
pub fn run_scope(s: &ScheduleRequest) -> Option<Map<String, Value>> {
    let identity = s.identity.as_ref()?;
    let worker = if s.worker_type.is_empty() {
        "default"
    } else {
        &s.worker_type
    };
    match json!({"folders": s.folders, "identity": identity, "worker_type": worker,
                 "profile": ferb_orchestrator::py::strip(&s.profile),
                 "account": ferb_orchestrator::py::strip(&s.account)})
    {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// `SendScope.model_dump()`.
pub fn dump_scope(s: &SendScope) -> Value {
    json!({"capability": s.capability, "identity": s.identity, "account": s.account, "site": s.site,
           "recipients": s.recipients, "content": s.content})
}

fn policy_scope(s: &SendScope) -> ferb_policy::standing::SendScope {
    ferb_policy::standing::SendScope {
        capability: s.capability.clone(),
        identity: s.identity.clone(),
        account: s.account.clone(),
        site: s.site.clone(),
        recipients: s.recipients.clone(),
        content: s.content.clone(),
    }
}

fn quoted(words: &str, quote: &str) -> bool {
    ferb_memory::terms::quoted(words, quote)
}

/// `normalize_path(p, workspace)`.
fn normalize(ctx: &Ctx<'_>, p: &str) -> String {
    ferb_fs::files::normalize_path(p, ctx.workspace, ctx.home).unwrap_or_else(|_| p.to_string())
}

/// `{**targets, ...}` as `Targets.of(...).to_json()`: each kind sorted, as strings.
fn targets_json(ctx: &Ctx<'_>, t: &ferb_orchestrator::models::TargetsSpec) -> Map<String, Value> {
    let set = |v: Vec<String>| -> Vec<String> {
        let mut s: Vec<String> = v.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
        s.sort();
        s
    };
    let paths: Vec<String> = t.paths.iter().map(|p| normalize(ctx, p)).collect();
    match json!({"apps": set(t.apps.clone()), "sites": set(t.sites.clone()), "paths": set(paths),
                 "recipients": set(t.recipients.clone())})
    {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// `_schedule_scopes`: the stated scopes a schedule records (redacted), and the lines of those it doesn't: a scope is
/// recorded only when its site, account and recipients appear in what the owner typed, in this turn or behind a
/// remembered item (D30, S3, MP2).
pub fn scopes(
    ctx: &Ctx<'_>,
    stated: &[SendScope],
    own_words: &str,
    request_id: &str,
) -> (Vec<Value>, Vec<String>) {
    let mut text = vec![own_words.to_string()];
    if !stated.is_empty() {
        text.extend((ctx.typed_behind_memory)(request_id));
    }
    let owner_text = text.join("\n");
    let (mut recorded, mut unnamed) = (Vec::new(), Vec::new());
    for scope in stated {
        if ferb_policy::standing::owner_named(&policy_scope(scope), &owner_text) {
            recorded.push(ctx.redactor.value(&dump_scope(scope)));
        } else {
            unnamed.push(
                ctx.redactor
                    .text(&ferb_orchestrator::steering::scope_line(scope)),
            );
        }
    }
    (recorded, unnamed)
}

/// `_book`.
fn book(
    ctx: &Ctx<'_>,
    s: &ferb_clock::schedules::Schedule,
    due: &Time,
    return_after: Option<&Time>,
) -> Result<ferb_clock::timers::Timer, String> {
    (ctx.book)(s, due, return_after)
}

fn label(s: &ferb_clock::schedules::Schedule) -> String {
    ferb_orchestrator::names::shown_name(Some(&s.name), &s.request)
}

/// `_other_times_line` (ORCH-555).
fn other_times_line(plan: &Plan) -> String {
    format!(
        "It runs {}; {} in its instructions {} not a time it runs.",
        plan.schedule.when_said(),
        said_list(&plan.missing),
        if plan.missing.len() == 1 { "is" } else { "are" }
    )
}

fn local(ctx: &Ctx<'_>, t: &Time) -> Time {
    ctx.clock.zone().local(t)
}

/// `f"{local:%a %d %b %H:%M}"`.
fn fmt(t: &Time, pattern: &str) -> String {
    cpy::strftime(&t.naive_local(), t.offset().local_minus_utc(), "", pattern)
}

/// supervisor's `_schedule_create` port (areas/schedules.py:238): from chat, the owner's words (quote-checked), the
/// targets recorded now, the first run in the future. The owner's line and, for a schedule set, its event card.
pub fn create(
    ctx: &Ctx<'_>,
    spec: &ScheduleRequest,
    own_words: &str,
    request_id: &str,
    origins: &BTreeSet<String>,
    request_origins: &BTreeSet<String>,
) -> (String, Option<Value>) {
    if !quoted(own_words, &spec.quote) {
        (ctx.journal)(
            "schedule.refused",
            json!({"why": "quote not in the owner's words"}),
            None,
        );
        return (
            blocked::blocked(
                blocked::REMINDERS,
                blocked::NOT_IN_WORDS,
                &format!("“{}” isn't scheduled", spec.request),
                "type what you want scheduled and I'll set it up",
            ),
            None,
        );
    }
    if let Some(ends) = &spec.until {
        if !quoted(own_words, &ends.quote) {
            (ctx.journal)(
                "schedule.refused",
                json!({"why": "end date words not in the owner's words"}),
                None,
            );
            return (
                blocked::blocked(
                    blocked::END_DATE,
                    "the end date words I quoted from you aren't in what you wrote",
                    &format!("“{}” isn't scheduled", spec.request),
                    "say when it should stop and I'll set it up",
                ),
                None,
            );
        }
    }
    let now = ctx.clock.now();
    let plan = match plan_new(&spec_of(spec), &now, &(ctx.new_id)(), &ctx.clock.zone()) {
        Ok(p) => p,
        Err(e) => return (format!("I couldn't schedule that: {e}."), None),
    };
    let mut targets = targets_json(ctx, &spec.targets);
    let (scopes, unnamed) = scopes(ctx, &spec.standing_sends, own_words, request_id);
    if !scopes.is_empty() {
        targets.insert("standing_sends".into(), Value::Array(scopes.clone()));
    }
    if let Some(mut scope) = run_scope(spec) {
        let mut folders: Vec<String> = Vec::new();
        for f in &spec.folders {
            if ferb_orchestrator::py::strip(f).is_empty() {
                continue;
            }
            let n = normalize(ctx, f);
            if !folders.contains(&n) {
                folders.push(n);
            }
        }
        scope.insert("folders".into(), json!(folders));
        targets.insert(RUN_SCOPE.into(), ctx.redactor.value(&Value::Object(scope)));
    }
    if let Some(t) = &plan.timing {
        targets.insert(TIMING.into(), t.to_json());
    }
    let added = ctx.schedules.add(
        &spec.request,
        own_words,
        &targets,
        &plan.rule,
        &plan.first,
        &spec.watch,
        plan.until.as_ref(),
        &spec.name,
    );
    let s = match added {
        Ok(s) => s,
        Err(e) => return (format!("I couldn't schedule that: {e}."), None),
    };
    // T-1066: its first run, its timing's earlier times included
    let first = plan.booked;
    let t = match book(ctx, &s, &first, plan.return_after.as_ref()) {
        Ok(t) => t,
        Err(e) => return (format!("I couldn't schedule that: {e}."), None),
    };
    let _ = ctx.schedules.set_timer(&s.id, Some(&t.id), Some(&first));
    let mut row = json!({"schedule_id": s.id, "request": s.request, "rule": s.when_said(),
        "request_id": request_id, "standing_sends": scopes, "not_recorded": unnamed, "watch": s.watch,
        "until": s.until.as_ref().map(cpy::iso), RUN_SCOPE: s.run_scope()});
    if !request_origins.is_empty() {
        row["request_origins"] = json!(request_origins.iter().collect::<Vec<_>>());
    }
    let made = origin::join(origins.iter().map(String::as_str));
    (ctx.journal)("schedule.created", row, Some(&made));
    // the owner's line in plain words (L11); the schedule's id stays in the journal row above
    let at_local = local(ctx, &first);
    let midnight =
        chrono::Timelike::hour(&at_local) == 0 && chrono::Timelike::minute(&at_local) == 0;
    let at = if plan.rule.when_active && midnight {
        fmt(&at_local, "%a %d %b")
    } else {
        fmt(&at_local, "%a %d %b %H:%M")
    };
    let timed = plan.rule.repeat != "none" || s.timing().is_some();
    let when = if timed {
        format!("{}, first on {at}", s.when_said())
    } else if !plan.rule.when_active {
        format!("on {at}")
    } else if plan
        .return_after
        .as_ref()
        .is_some_and(|r| *r <= cpy::plus_us(&now, 1_000_000).unwrap_or(now))
    {
        "the next time you're back at your Mac".into()
    } else {
        format!("on {at}, when you're at your Mac")
    };
    let zone = ctx.clock.zone();
    let until = s
        .until
        .as_ref()
        .map(|u| until_said(u, &zone))
        .unwrap_or_default();
    let mut said = [
        format!("🗓 Scheduled: {} — {when}", label(&s)),
        until,
        watch_said(&s.watch).to_string(),
    ]
    .into_iter()
    .filter(|p| !p.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    let repeat = s.when_said();
    let card = ferb_orchestrator::cards::event(
        "schedule",
        &s.id,
        &label(&s),
        &cpy::iso(&first),
        &said,
        timed.then_some(repeat.as_str()),
        Some(watch_said(&s.watch)).filter(|w| !w.is_empty()),
        s.until.as_ref().map(cpy::iso).as_deref(),
        None,
    );
    if let Some(scope) = s.run_scope() {
        // ORCH-313: stated now, so the owner can correct it
        let mut shown = scope.clone();
        if shown.get("identity").and_then(Value::as_str) == Some("ferb") {
            if let Some(name) = ctx.browsing.and_then(|b| b.as_owner(request_id)) {
                shown.insert("identity".into(), json!("owner"));
                shown.insert("profile".into(), json!(name));
            }
        }
        said.push('\n');
        said.push_str(&ferb_policy::confirm::from_task_lines(&scope_said(
            &shown,
            ctx.home,
            ctx.worker_label,
        )));
    }
    if !scopes.is_empty() {
        let lines: Vec<String> = spec
            .standing_sends
            .iter()
            .filter(|sc| scopes.contains(&ctx.redactor.value(&dump_scope(sc))))
            .map(|sc| format!("- {}", ferb_orchestrator::steering::scope_line(sc)))
            .collect();
        said.push('\n');
        said.push_str(&ferb_policy::confirm::from_task_lines(&format!(
            "Each run will send these without asking each time:\n{}",
            lines.join("\n")
        )));
    }
    if !unnamed.is_empty() {
        said.push('\n');
        said.push_str(&ferb_policy::confirm::from_task_lines(&format!(
            "These weren't recorded, because their destination isn't in words you typed (now, or when you told me \
             to remember it). In each run they get FERB's usual checks, and a send that asks can't be answered with \
             nobody there, so it's reported not done:\n{}",
            unnamed
                .iter()
                .map(|l| format!("- {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        )));
    }
    if !plan.missing.is_empty() {
        said.push('\n');
        said.push_str(&other_times_line(&plan));
    }
    if s.run_scope()
        .and_then(|r| r.get("identity").cloned())
        .is_some_and(|i| i == "ferb")
    {
        if let Some(b) = ctx.browsing {
            b.ask(request_id);
        }
    }
    (said, Some(card))
}

/// `_schedule_problems` (T-1066): what code finds wrong with a new or changed schedule before the planner's reply
/// stands: timing code can't work out, or a clock time its request names that it would never run at.
pub fn problems(ctx: &Ctx<'_>, entry: &Entry) -> Vec<String> {
    let now = ctx.clock.now();
    let (what, plan) = match entry {
        Entry::New(sched) => {
            let what = format!(
                "schedule {}",
                cpy::repr(&sched.request.chars().take(60).collect::<String>())
            );
            (
                what,
                plan_new(&spec_of(sched), &now, &(ctx.new_id)(), &ctx.clock.zone()),
            )
        }
        Entry::Change(ch) => {
            if ch.action != "edit" {
                return Vec::new();
            }
            let what = format!("schedule_changes {}", cpy::repr(&ch.id));
            let old = ctx.schedules.get(&ch.id).ok().flatten();
            let Some(old) = old.filter(|o| o.state == "active" || o.state == "paused") else {
                return vec![format!(
                    "{what}: there's no active or paused schedule with that id to edit."
                )];
            };
            if ferb_orchestrator::py::strip(&ch.request).is_empty() && ch.timing.repeat.is_empty() {
                return vec![format!(
                    "{what}: an edit changes its request, its timing or both."
                )];
            }
            (
                what,
                plan_edit(&old, &ch.request, &timing_of(&ch.timing), &now, "check"),
            )
        }
    };
    let plan = match plan {
        Ok(p) => p,
        Err(e) => return vec![format!("{what}: {e}.")],
    };
    if !plan.missing.is_empty() {
        return vec![format!(
            "{what}: its request names {}, but its saved timing would run {}. Every clock time a schedule's request \
             names should be a time it runs at (when, times, random) or its end: set the timing to every time the \
             owner asked for. A time that isn't a run time stays, and FERB says it isn't one.",
            plan.missing.join(", "),
            plan.schedule.when_said()
        )];
    }
    Vec::new()
}

/// `_schedule_edit_from_chat` (T-1066): an active or paused schedule changed in place, on the owner's quoted words: its
/// instructions, its timing or both, with the same id, runs and history.
pub fn edit(
    ctx: &Ctx<'_>,
    ch: &ScheduleChange,
    own_words: &str,
    request_id: &str,
    origins: &BTreeSet<String>,
    request_origins: &BTreeSet<String>,
) -> (String, Option<Value>) {
    if !quoted(own_words, &ch.quote) {
        return (
            blocked::blocked(
                blocked::CHANGES,
                blocked::NOT_IN_WORDS,
                "no schedule was changed",
                "say which schedule should change and how",
            ),
            None,
        );
    }
    let old = ctx.schedules.get(&ch.id).ok().flatten();
    let Some(old) = old.filter(|o| o.state == "active" || o.state == "paused") else {
        return (
            "There's no active schedule like that to change.".into(),
            None,
        );
    };
    if ferb_orchestrator::py::strip(&ch.request).is_empty() && ch.timing.repeat.is_empty() {
        return (
            "I didn't change that schedule — could you say again what should change?".into(),
            None,
        );
    }
    let now = ctx.clock.now();
    let plan = match plan_edit(
        &old,
        &ch.request,
        &timing_of(&ch.timing),
        &now,
        &(ctx.new_id)(),
    ) {
        Ok(p) => p,
        Err(e) => return (format!("I didn't change “{}”: {e}.", label(&old)), None),
    };
    let retimed = !ch.timing.repeat.is_empty();
    let targets = retimed.then(|| {
        let mut t: Map<String, Value> = old
            .targets
            .iter()
            .filter(|(k, _)| k.as_str() != TIMING)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Some(timing) = &plan.timing {
            t.insert(TIMING.into(), timing.to_json());
        }
        t
    });
    let request = (plan.schedule.request != old.request).then_some(plan.schedule.request.as_str());
    let _ = ctx.schedules.edit(
        &old.id,
        request,
        if retimed {
            Some((&plan.rule, &plan.first))
        } else {
            None
        },
        targets.as_ref(),
        own_words,
    );
    let renamed = ch.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if !renamed.is_empty() {
        let _ = ctx.schedules.set_name(&old.id, &renamed);
    }
    if retimed && old.state == "active" {
        if let Some(tid) = &old.timer_id {
            let _ = ctx.timers.cancel(tid);
        }
        if let Ok(booked) = book(ctx, &plan.schedule, &plan.booked, None) {
            let _ = ctx
                .schedules
                .set_timer(&old.id, Some(&booked.id), Some(&plan.booked));
        }
    }
    let Some(s) = ctx.schedules.get(&old.id).ok().flatten() else {
        return (
            "There's no active schedule like that to change.".into(),
            None,
        );
    };
    let (before, after) = (old.when_said(), s.when_said());
    let mut row = json!({"schedule_id": s.id, "request_id": request_id,
        "before": {"request": old.request, "rule": before}, "after": {"request": s.request, "rule": after}});
    if !request_origins.is_empty() {
        row["request_origins"] = json!(request_origins.iter().collect::<Vec<_>>());
    }
    let made = origin::join(origins.iter().map(String::as_str));
    (ctx.journal)("schedule.edited", row, Some(&made));
    let next = match (&s.next_run, s.state.as_str()) {
        (Some(n), "active") => format!("; next run {}", fmt(&local(ctx, n), "%a %d %b %H:%M")),
        (_, "paused") => "; it stays paused".into(),
        _ => String::new(),
    };
    let timing = if after != before {
        format!("now {after} (was {before})")
    } else {
        format!("still {after}")
    };
    let zone = ctx.clock.zone();
    let line = [
        format!("🗓 Changed: {} — {timing}{next}", label(&s)),
        s.until
            .as_ref()
            .map(|u| until_said(u, &zone))
            .unwrap_or_default(),
        watch_said(&s.watch).to_string(),
    ]
    .into_iter()
    .filter(|p| !p.is_empty())
    .collect::<Vec<_>>()
    .join(", ");
    let mut said = line.clone();
    if s.request != old.request {
        said.push_str(&format!("\nIts instructions now: {}", s.request));
    }
    if !plan.missing.is_empty() {
        said.push('\n');
        said.push_str(&other_times_line(&plan));
    }
    let mut card = ferb_orchestrator::cards::event(
        "schedule",
        &s.id,
        &label(&s),
        &s.next_run.as_ref().map(cpy::iso).unwrap_or_default(),
        &line,
        Some(after.as_str()),
        Some(watch_said(&s.watch)).filter(|w| !w.is_empty()),
        s.until.as_ref().map(cpy::iso).as_deref(),
        None,
    );
    card["changed"] = json!(true);
    if before != after {
        card["was_repeat"] = json!(before);
    }
    if s.request != old.request {
        card["was_request"] = json!(old.request);
        card["request"] = json!(s.request);
    }
    (said, Some(card))
}
