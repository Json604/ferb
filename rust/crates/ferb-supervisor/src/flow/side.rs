//! A planner answer's side entries (supervisor.py:3344 `_side_entries`, 1439 `_side_quote_problems`): the reminders
//! and timers the owner asked for in chat (`_set_timers`, `_reminder_set`, `_timer_said`, `_asked`; L11, ORCH-590,
//! ORCH-519, ORCH-520, T-1055), each with the one quote check against the owner's words, set on FERB's timer store
//! (ferb-clock's `Timers`) and said in code's own lines and cards.
//!
//! The other side entries (memory, forgets, reminder cancels, schedules and their changes, the schedules card,
//! conversation changes) answer `NotPorted` by name in turn.rs.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_clock::py::Time;
use ferb_clock::timers::{Timer, Timers};
use ferb_orchestrator::models::{SupervisorOutcome, TimerRequest};
use ferb_policy::small::blocked;
use serde_json::{json, Value};

use super::{Inner, Turn};
use crate::lines::{already_set, not_set, quoted as quoted_names, reminder_line, TimerFacts};
use crate::words::RECENT_EXCHANGES;

/// What the timers of one planner answer did (the timers' part of `_Side`).
#[derive(Default)]
pub struct TimersSide {
    pub events: Vec<Value>,
    pub lines: Vec<String>,
    pub refused: Vec<(String, String)>,
    pub cards: Vec<Value>,
}

/// One owner message as a reminder's quote check reads it: (its request id, its words).
type Said = (String, String);

/// supervisor.py:299 `_overlap`: two reminders' normalized words name the same request when one lies within the other.
fn overlap(a: &str, b: &str) -> bool {
    a.contains(b) || b.contains(a)
}

/// `re.split(r"\n[ \t]*\n", quote)`'s non-blank parts.
fn parts(quote: &str) -> Vec<String> {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\n[ \t]*\n").expect("a pattern"));
    RE.split(quote)
        .filter(|p| !ferb_orchestrator::py::strip(p).is_empty())
        .map(str::to_string)
        .collect()
}

/// `items.quoted(words, quote)`: the one quote check.
fn quoted(words: &str, quote: &str) -> bool {
    ferb_memory::terms::quoted(words, quote)
}

/// supervisor.py:336 `_asked`: the request ids of the owner's messages a reminder's quote is whole words of; a quote of
/// several messages joined by blank lines counts when every part is in this request's chain of owner words, and the
/// message holding its first part asked it. [] when none: the quote check refuses.
pub fn asked(said: &[Said], chain: &str, quote: &str) -> Vec<String> {
    let mut asked: Vec<String> = said
        .iter()
        .filter(|(_, words)| quoted(words, quote))
        .map(|(s, _)| s.clone())
        .collect();
    let parts = parts(quote);
    if asked.is_empty() && parts.len() > 1 && !said.is_empty() && quoted(chain, quote) {
        asked = vec![said
            .iter()
            .find(|(_, words)| quoted(words, &parts[0]))
            .map(|(s, _)| s.clone())
            .unwrap_or_else(|| said[0].0.clone())];
    }
    asked
}

/// The planner's `When` as ferb-clock reads it.
fn when_of(it: &TimerRequest) -> ferb_clock::when::When {
    ferb_clock::when::When {
        in_minutes: it.when.in_minutes,
        in_days: it.when.in_days.as_ref().and_then(|d| d.0.parse().ok()),
        day: it.when.day.clone(),
        date: it.when.date.clone(),
        time: it.when.time.clone(),
    }
}

fn facts(t: &Timer) -> TimerFacts {
    TimerFacts {
        fire_at: t.fire_at,
        kind: t.kind.clone(),
        message: t.message.clone(),
        state: t.state.clone(),
    }
}

impl Inner {
    /// FERB's timer store on the supervisor's database (`self.timers`, the schedules port's).
    fn timers<T>(&self, f: impl FnOnce(&Timers<'_>) -> T) -> T {
        let db = self.db();
        let clock = self.deps.clock.clone();
        let ids = self.deps.ids.clone();
        let timers = Timers::new(&db, clock.as_ref(), move || ids.new_id());
        f(&timers)
    }

    /// supervisor.py:293 `_reminder_time`: when a reminder is due, and the return to the Mac it waits for (ORCH-590).
    fn reminder_time(
        &self,
        it: &TimerRequest,
        now: &Time,
        present: bool,
    ) -> Result<(Time, Option<Time>), ferb_clock::when::WhenError> {
        let zone = self.deps.clock.zone();
        let w = when_of(it);
        if present {
            ferb_clock::when::when_present(&w, "none", now, None, &zone)
        } else {
            Ok((ferb_clock::when::resolve(&w, now, &zone)?, None))
        }
    }

    /// supervisor.py:1407 `_timer_said`: this turn's own message alone first, then this chat's recent earlier owner
    /// messages (`_owner_words_in_chat`), newest first, each with its request.
    pub fn timer_said(&self, turn: &Arc<Mutex<Turn>>) -> Vec<Said> {
        let (rid, own, session, current) = {
            let t = turn.lock().unwrap();
            (
                t.request_id.clone(),
                t.own_words.clone(),
                t.session.clone(),
                t.request_ids(),
            )
        };
        let mut earlier: Vec<Option<Said>> = Vec::new();
        for entry in self.of_kind("chat.message", "session_id", &session, Some("owner")) {
            if current.contains(&entry.request_id) {
                continue;
            }
            let held: BTreeSet<String> = ferb_sanitize::origin::split(Some(&entry.origin))
                .into_iter()
                .collect();
            earlier.push(
                if !held.contains(ferb_sanitize::origin::OWNER)
                    || held.contains(&ferb_sanitize::origin::external("unlabeled", ""))
                {
                    None
                } else {
                    Some((
                        entry.request_id.clone(),
                        ferb_sanitize::untrusted::typed_lines_of(&entry.text("text")),
                    ))
                },
            );
        }
        std::iter::once((rid, own))
            .chain(earlier.into_iter().rev().take(RECENT_EXCHANGES).flatten())
            .collect()
    }

    /// supervisor.py:1346 `_reminder_set`: the reminder already set from request `source` for these words at the time
    /// the planner's `when` means, whatever became of it; None when there is none.
    fn reminder_set(
        &self,
        source: &str,
        quote: &str,
        it: &TimerRequest,
        present: bool,
    ) -> Option<Timer> {
        let recorded: Vec<(String, Value)> = self
            .of_kind("timer.set", "request_id", source, None)
            .into_iter()
            .map(|e| (e.text("timer_id"), e.data))
            .collect();
        let ids: Vec<String> = {
            let db = self.db();
            let mut st = db
                .conn()
                .prepare(
                    "SELECT id FROM timers WHERE kind IN ('reminder', 'timer') AND request_id = ? \
                     ORDER BY created_at",
                )
                .ok()?;
            let rows = st
                .query_map([source], |r| r.get::<_, String>(0))
                .ok()?
                .flatten()
                .collect();
            rows
        };
        for id in ids {
            // the last record of that timer wins, as a dict built from the rows does
            let data = recorded
                .iter()
                .rev()
                .find(|(t, _)| *t == id)
                .map(|(_, d)| d.clone());
            if let Some(d) = &data {
                let earlier = d
                    .get("quote")
                    .map(ferb_py::value::py_str)
                    .unwrap_or_default();
                let earlier = if d.get("quote").is_some_and(ferb_py::value::truthy) {
                    earlier
                } else {
                    String::new()
                };
                if !overlap(&earlier, quote) {
                    continue;
                }
            }
            let timer = self.timers(|t| t.get(&id)).ok().flatten();
            let moment = data
                .as_ref()
                .and_then(|d| d.get("resolved_from"))
                .filter(|m| !m.is_null());
            let Some(moment) = moment else {
                return timer;
            };
            let Some(from) = ferb_clock::py::parse(&ferb_py::value::py_str(moment)) else {
                continue;
            };
            match self.reminder_time(it, &from, present) {
                Ok((at, _)) if timer.as_ref().is_some_and(|t| t.fire_at == at) => return timer,
                _ => continue,
            }
        }
        None
    }

    /// supervisor.py:1273 `_set_timers`: the reminders the owner asked for in chat, kept only when the quote is whole
    /// words of one of the owner's messages and the time is in the future; a message authorizes its reminder once
    /// (L11). Returns the reminders set, those refused with code's reason, those already there, and the quote check's
    /// refusals as (message, quote).
    #[allow(clippy::type_complexity)]
    fn set_timers(
        &self,
        items: &[TimerRequest],
        said: &[Said],
        request_id: &str,
        chain: &str,
    ) -> (
        Vec<Timer>,
        Vec<(String, String)>,
        Vec<Timer>,
        Vec<(String, String)>,
    ) {
        let now = self.deps.clock.now();
        let mut out: Vec<Timer> = Vec::new();
        let mut refused: Vec<(String, String)> = Vec::new();
        let mut stopped: Vec<(String, String)> = Vec::new();
        let mut existing: Vec<Timer> = Vec::new();
        let mut made: Vec<(Timer, String)> = Vec::new();
        let exists = |existing: &mut Vec<Timer>, t: Timer| {
            if existing.iter().all(|e| e.id != t.id) {
                existing.push(t);
            }
        };
        for it in items {
            let quote = ferb_orchestrator::py::split_ws(&it.quote.to_lowercase()).join(" ");
            let asked = asked(said, chain, &it.quote);
            if asked.is_empty() {
                self.journal(
                    "timer.refused",
                    json!({"why": "quote not in the owner's words"}),
                    None,
                );
                stopped.push((it.message.clone(), it.quote.clone()));
                continue;
            }
            let present = it.when_active;
            let (at, back, late) = match self.reminder_time(it, &now, present) {
                Ok((at, back)) => (Some(at), back, None),
                Err(e) => (None, None, Some(e)),
            };
            if let Some((twin, _)) = made
                .iter()
                .find(|(t, words)| overlap(words, &quote) && Some(t.fire_at) == at)
            {
                self.journal(
                    "timer.refused",
                    json!({"why": "already set in this turn"}),
                    None,
                );
                exists(&mut existing, twin.clone());
                continue;
            }
            let found: Vec<(String, Option<Timer>)> = asked
                .iter()
                .map(|s| (s.clone(), self.reminder_set(s, &quote, it, present)))
                .collect();
            let source = found
                .iter()
                .find(|(_, f)| f.is_none())
                .map(|(s, _)| s.clone());
            let Some(source) = source else {
                self.journal(
                    "timer.refused",
                    json!({"why": "already set from that message"}),
                    None,
                );
                if let Some(t) = found[0].1.clone() {
                    exists(&mut existing, t);
                }
                continue;
            };
            if let Some(e) = late {
                self.journal("timer.refused", json!({"why": e.to_string()}), None);
                refused.push((it.message.clone(), e.to_string()));
                continue;
            }
            let at = at.expect("a time");
            let kind = if it.kind == "timer" {
                "timer"
            } else {
                "reminder"
            };
            let added = self.timers(|t| {
                t.add(
                    kind,
                    &at,
                    &it.message,
                    None,
                    Some(&source),
                    present,
                    back.as_ref(),
                    &it.name,
                    None,
                )
            });
            let t = match added {
                Ok(t) => t,
                Err(e) => {
                    self.journal("timer.refused", json!({"why": e.to_string()}), None);
                    refused.push((it.message.clone(), e.to_string()));
                    continue;
                }
            };
            let mut data = json!({"timer_id": t.id, "request_id": source, "quote": quote, "set_in": request_id,
                "resolved_from": ferb_clock::py::isoformat(&now, ferb_clock::py::Spec::Auto)});
            if present {
                data["when_active"] = json!(true);
            }
            self.journal("timer.set", data, None);
            out.push(t.clone());
            made.push((t, quote));
        }
        (out, refused, existing, stopped)
    }

    /// The timers' part of supervisor.py:3344 `_side_entries`: each reminder set says so in code's words (its event,
    /// its line and its card), each refusal says why (ORCH-519's safety-check line for the quote check).
    pub fn timer_side(&self, turn: &Arc<Mutex<Turn>>, out: &SupervisorOutcome) -> TimersSide {
        let mut side = TimersSide::default();
        if out.timers.is_empty() {
            return side;
        }
        let (rid, typed) = {
            let t = turn.lock().unwrap();
            (t.request_id.clone(), t.typed.clone())
        };
        let said = self.timer_said(turn);
        let (set, unset, existing, unquoted) = self.set_timers(&out.timers, &said, &rid, &typed);
        let stopped: Vec<(String, String)> = unquoted
            .iter()
            .map(|(message, _)| {
                (
                    "timer".to_string(),
                    blocked::blocked(
                        blocked::REMINDERS,
                        blocked::NOT_IN_RECENT,
                        &format!("the reminder {} isn't set", quoted_names(&[message])),
                        "type the reminder in this chat and I'll set it",
                    ),
                )
            })
            .collect();
        let zone = self.deps.clock.zone();
        let now = self.deps.clock.now();
        let active = |t: &Timer| self.timers(|s| s.when_active(&t.id)).unwrap_or(false);
        for t in &set {
            let line = reminder_line(&facts(t), active(t), Some(&now), &zone);
            side.events
                .push(json!({"type": "timer", "text": line, "timer_id": t.id}));
            let started = (t.kind == "timer")
                .then(|| {
                    t.created_at
                        .map(|c| ferb_clock::py::isoformat(&c, ferb_clock::py::Spec::Auto))
                })
                .flatten();
            side.cards.push(ferb_orchestrator::cards::event(
                if t.kind == "timer" {
                    "timer"
                } else {
                    "reminder"
                },
                &t.id,
                &ferb_orchestrator::names::shown_name(Some(&t.name), &t.message),
                &ferb_clock::py::isoformat(&t.fire_at, ferb_clock::py::Spec::Auto),
                &line,
                None,
                None,
                None,
                started.as_deref(),
            ));
        }
        let mut refused: Vec<(String, String)> = std::iter::once(not_set(&unset))
            .chain(existing.iter().map(|t| already_set(&facts(t), &zone)))
            .filter(|l| !l.is_empty())
            .map(|l| ("timer".to_string(), l))
            .collect();
        refused.extend(stopped);
        side.lines = set
            .iter()
            .map(|t| reminder_line(&facts(t), active(t), Some(&now), &zone))
            .chain(refused.iter().map(|(_, l)| l.clone()))
            .collect();
        side.refused = refused;
        side
    }

    /// The timers' part of supervisor.py:1439 `_side_quote_problems`.
    pub fn timer_quote_problems(
        &self,
        turn: &Arc<Mutex<Turn>>,
        out: &SupervisorOutcome,
    ) -> Vec<String> {
        if out.timers.is_empty() {
            return Vec::new();
        }
        let typed = turn.lock().unwrap().typed.clone();
        let said = self.timer_said(turn);
        out.timers
            .iter()
            .filter(|it| asked(&said, &typed, &it.quote).is_empty())
            .map(|it| {
                format!(
                    "timer {}: its quote isn't the owner's exact words from this message or their recent messages \
                     in this chat, so code would refuse it: {}",
                    ferb_orchestrator::py::repr_str(&it.message),
                    ferb_py::json::dumps_utf8(&json!(it.quote))
                )
            })
            .collect()
    }
}
