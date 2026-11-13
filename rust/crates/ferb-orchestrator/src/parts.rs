//! ferb/orchestrator/parts.py: answers in steps (ORCH-319). One card per multi-part turn, a row per part, each
//! answer posted once, durably (`chat.part` journal rows), never twice, also across a restart. No model writes here:
//! code records and counts.
//!
//! decision: ORCH-319, ORCH-332, ORCH-535, ORCH-543, ORCH-613

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::lines::shorten;
use crate::models::Part;
use crate::ports::Journal;
use crate::py::strip;

pub const KIND: &str = "chat.part";
pub const DONE: &[&str] = &["done", "updated"];
pub const STOPPED: &str = "stopped";
pub const UPDATED_WHY: &str = "Updated";
pub const WHY_CORRECTION: &str = "done after your correction";
pub const WHY_FOLLOW_UP: &str = "changed after your follow-up";
pub const NOT_FINISHED: &str = "not finished";
pub const PARTLY_UNCONFIRMED: &str = "not all of it confirmed";
pub const RESULTS: &str = "Results: ";

/// The wall clock in UTC, as `datetime.now(timezone.utc)` (injected: tests freeze it).
pub type UtcNow = Arc<dyn Fn() -> ferb_py::time::DateTime + Send + Sync>;

fn s(v: &Value) -> String {
    ferb_py::value::py_str(v)
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

fn state_in(row: Option<&Map<String, Value>>, states: &[&str]) -> bool {
    row.and_then(|r| r.get("state"))
        .and_then(|v| v.as_str())
        .is_some_and(|st| states.contains(&st))
}

/// `v not in (None, "", False)` (0 and 0.0 equal False too).
fn kept_field(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        _ => true,
    }
}

/// `v not in (None, "", [], {})`.
fn shown_field(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

/// parts.py:29 `Card`.
pub struct Card {
    pub journal: Rc<dyn Journal>,
    pub task_id: String,
    pub request_id: String,
    pub parts: Vec<Part>,
    pub emit: Arc<dyn Fn(Value) + Send + Sync>,
    pub origin: Arc<dyn Fn() -> String + Send + Sync>,
    pub now: UtcNow,
    live: HashMap<String, Map<String, Value>>,
    seq: String,
    pub here: Vec<String>,
}

impl Card {
    pub fn new(
        journal: Rc<dyn Journal>,
        task_id: &str,
        request_id: &str,
        parts: Vec<Part>,
        emit: Arc<dyn Fn(Value) + Send + Sync>,
        origin: Arc<dyn Fn() -> String + Send + Sync>,
        now: UtcNow,
    ) -> Card {
        let mut card = Card {
            journal,
            task_id: task_id.to_string(),
            request_id: request_id.to_string(),
            parts,
            emit,
            origin,
            now,
            live: HashMap::new(),
            seq: String::new(),
            here: Vec::new(),
        };
        let records = card.records();
        card.here = card
            .parts
            .iter()
            .filter(|p| !state_in(records.get(&p.id), DONE))
            .map(|p| p.id.clone())
            .collect();
        card
    }

    pub fn part_of(&self, step_id: &str) -> Option<&Part> {
        self.parts
            .iter()
            .find(|p| p.answer_step.as_deref() == Some(step_id))
    }

    /// parts.py:51 `records`: each part's latest durable row of this task (same id and quote).
    pub fn records(&self) -> HashMap<String, Map<String, Value>> {
        let quotes: HashMap<&str, &str> = self
            .parts
            .iter()
            .map(|p| (p.id.as_str(), p.quote.as_str()))
            .collect();
        let mut latest: HashMap<String, Map<String, Value>> = HashMap::new();
        for entry in self.journal.of_kind(KIND, "task_id", &self.task_id) {
            let Some(data) = entry.data.as_object() else {
                continue;
            };
            let part = data.get("part").map(s).unwrap_or_else(|| "None".into());
            if let Some(quote) = quotes.get(part.as_str()) {
                let same = match data.get("quote") {
                    None => true,
                    Some(q) => q.as_str() == Some(*quote),
                };
                if same {
                    latest.insert(part, data.clone());
                }
            }
        }
        latest
    }

    fn shown(&self, part: &Part, records: &HashMap<String, Map<String, Value>>) -> bool {
        self.here.contains(&part.id)
            || records
                .get(&part.id)
                .and_then(|r| r.get("request_id"))
                .and_then(|v| v.as_str())
                == Some(self.request_id.as_str())
    }

    /// parts.py:66 `rows`.
    pub fn rows(&self) -> Vec<Map<String, Value>> {
        let records = self.records();
        let mut out = Vec::new();
        for p in &self.parts {
            if !self.shown(p, &records) {
                continue;
            }
            let mut row = Map::new();
            row.insert("part".into(), Value::String(p.id.clone()));
            row.insert("label".into(), Value::String(p.label.clone()));
            row.insert("state".into(), Value::String("working".into()));
            row.insert("direct".into(), Value::Bool(p.direct.is_some()));
            if let Some(step) = p.answer_step.as_ref().filter(|x| !x.is_empty()) {
                row.insert("step".into(), Value::String(step.clone()));
            }
            let posted = records.get(&p.id);
            let live = self.live.get(&p.id);
            let live_newer = live.is_some_and(|l| {
                posted.is_none()
                    || l.get("seq").map(s).unwrap_or_default()
                        > posted.and_then(|x| x.get("seq")).map(s).unwrap_or_default()
            });
            if live_newer {
                for (k, v) in live.expect("live") {
                    if !matches!(k.as_str(), "type" | "task_id" | "request_id") {
                        row.insert(k.clone(), v.clone());
                    }
                }
            } else if let Some(posted) = posted {
                for k in [
                    "state",
                    "text",
                    "note",
                    "why",
                    "seq",
                    "verified",
                    "card_request_id",
                ] {
                    if truthy(posted.get(k)) {
                        row.insert(k.into(), posted[k].clone());
                    }
                }
            }
            out.push(row);
        }
        out
    }

    /// parts.py:85 `tally`: (done, total) over this card's rows.
    pub fn tally(&self) -> (usize, usize) {
        let records = self.records();
        let shown: Vec<&Part> = self
            .parts
            .iter()
            .filter(|p| self.shown(p, &records))
            .collect();
        let done = shown
            .iter()
            .filter(|p| p.direct.is_some() || state_in(records.get(&p.id), DONE))
            .count();
        (done, shown.len())
    }

    /// parts.py:95 `stopped`.
    pub fn stopped(&self) -> usize {
        let records = self.records();
        self.parts
            .iter()
            .filter(|p| {
                p.direct.is_none()
                    && self.shown(p, &records)
                    && state_in(records.get(&p.id), &[STOPPED])
            })
            .count()
    }

    /// parts.py:101 `line`.
    pub fn line(&self) -> String {
        let (done, total) = self.tally();
        done_line(done, total, self.stopped())
    }

    /// parts.py:106 `ended`.
    pub fn ended(&self) -> &'static str {
        let (done, total) = self.tally();
        let stopped = self.stopped();
        if done + stopped < total {
            return "failed";
        }
        if done > 0 {
            "done"
        } else {
            "cancelled"
        }
    }

    /// parts.py:115 `unposted`.
    pub fn unposted(&self) -> Vec<Part> {
        let records = self.records();
        self.parts
            .iter()
            .filter(|p| {
                p.direct.is_none()
                    && self.here.contains(&p.id)
                    && records
                        .get(&p.id)
                        .and_then(|r| r.get("request_id"))
                        .and_then(|v| v.as_str())
                        != Some(self.request_id.as_str())
            })
            .cloned()
            .collect()
    }

    /// parts.py:123 `_next_seq` (ORCH-332): strictly increasing, also across a restart.
    fn next_seq(&mut self) -> String {
        let mut floor = self.seq.clone();
        for r in self.records().values() {
            let seq = r.get("seq").map(s).unwrap_or_default();
            if seq > floor {
                floor = seq;
            }
        }
        let mut now = (self.now)();
        loop {
            let stamp = now.isoformat(ferb_py::time::Timespec::Microseconds);
            if stamp > floor {
                self.seq = stamp.clone();
                return stamp;
            }
            now = now.plus(0, 1).expect("a time inside the calendar");
        }
    }

    /// parts.py:134 `say_direct`.
    pub fn say_direct(&mut self) -> Vec<Value> {
        let records = self.records();
        let mut said = Vec::new();
        for p in self.parts.clone() {
            let Some(direct) = &p.direct else { continue };
            if !self.here.contains(&p.id)
                || records
                    .get(&p.id)
                    .and_then(|r| r.get("request_id"))
                    .and_then(|v| v.as_str())
                    == Some(self.request_id.as_str())
            {
                continue;
            }
            self.record(&p, "done", direct, vec![("direct", Value::Bool(true))]);
            said.push(serde_json::json!({"part": p.id, "label": p.label, "text": direct}));
        }
        said
    }

    /// parts.py:146 `settle`: whether it posted.
    pub fn settle(
        &mut self,
        part: &Part,
        outcome: &str,
        text: &str,
        note: &str,
        verified: bool,
        step_task: &str,
    ) -> bool {
        let latest = self.records().get(&part.id).cloned();
        let Some(latest) = latest else {
            return self.post(part, outcome, text, note, "", verified, step_task);
        };
        let same_run =
            latest.get("request_id").and_then(|v| v.as_str()) == Some(self.request_id.as_str());
        let before = latest
            .get("state")
            .and_then(|v| v.as_str())
            .map(String::from);
        let before = before.as_deref();
        if outcome == "failed" || outcome == STOPPED {
            if matches!(before, Some("failed") | Some(STOPPED)) && !same_run {
                return self.post(part, outcome, text, "", "", false, step_task);
            }
            return false;
        }
        if before.is_some_and(|b| DONE.contains(&b))
            && (same_run || latest.get("text").and_then(|v| v.as_str()) == Some(text))
        {
            return false;
        }
        let why = if same_run {
            WHY_CORRECTION
        } else {
            WHY_FOLLOW_UP
        };
        self.post(part, "updated", text, note, why, verified, step_task)
    }

    /// parts.py:166 `update`.
    pub fn update(&mut self, part: &Part, text: &str, why: &str, step_task: &str) -> bool {
        let latest = self.records().get(&part.id).cloned();
        match latest {
            Some(l)
                if state_in(Some(&l), DONE)
                    && l.get("text").and_then(|v| v.as_str()) != Some(text) =>
            {
                self.post(part, "updated", text, "", why, false, step_task)
            }
            _ => false,
        }
    }

    fn record(
        &mut self,
        part: &Part,
        state: &str,
        text: &str,
        fields: Vec<(&str, Value)>,
    ) -> Map<String, Value> {
        let first = self
            .journal
            .of_kind(KIND, "task_id", &self.task_id)
            .into_iter()
            .find(|e| {
                e.data.get("part").and_then(|v| v.as_str()) == Some(part.id.as_str())
                    && match e.data.get("quote") {
                        None => true,
                        Some(q) => q.as_str() == Some(part.quote.as_str()),
                    }
            })
            .map(|e| e.data.get("request_id").cloned().unwrap_or(Value::Null))
            .unwrap_or_else(|| Value::String(self.request_id.clone()));
        let seq = self.next_seq();
        let mut data = Map::new();
        data.insert("task_id".into(), Value::String(self.task_id.clone()));
        data.insert("request_id".into(), Value::String(self.request_id.clone()));
        data.insert("part".into(), Value::String(part.id.clone()));
        data.insert("label".into(), Value::String(part.label.clone()));
        data.insert("quote".into(), Value::String(part.quote.clone()));
        data.insert("state".into(), Value::String(state.into()));
        data.insert("text".into(), Value::String(text.into()));
        data.insert("seq".into(), Value::String(seq));
        data.insert("card_request_id".into(), first);
        for (k, v) in fields {
            if kept_field(&v) {
                data.insert(k.into(), v);
            }
        }
        let origin = (self.origin)();
        self.journal
            .append(KIND, Value::Object(data.clone()), Some(&origin));
        data
    }

    #[allow(clippy::too_many_arguments)]
    fn post(
        &mut self,
        part: &Part,
        state: &str,
        text: &str,
        note: &str,
        why: &str,
        verified: bool,
        step_task: &str,
    ) -> bool {
        let step = part
            .answer_step
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null);
        let data = self.record(
            part,
            state,
            text,
            vec![
                ("note", Value::String(note.into())),
                ("why", Value::String(why.into())),
                ("verified", Value::Bool(verified)),
                ("step", step),
                ("step_task_id", Value::String(step_task.into())),
            ],
        );
        self.live.remove(&part.id);
        let mut event = Map::new();
        event.insert("type".into(), Value::String("part".into()));
        for (k, v) in data {
            if k != "quote" {
                event.insert(k, v);
            }
        }
        (self.emit)(Value::Object(event));
        true
    }

    /// parts.py:192 `show`: a live state of a row (an event, never recorded). `fields` in the caller's order.
    pub fn show(&mut self, part: &Part, state: &str, fields: Vec<(&str, Value)>) {
        let posted = self.records().get(&part.id).cloned().unwrap_or_default();
        if posted.get("request_id").and_then(|v| v.as_str()) == Some(self.request_id.as_str())
            && state_in(Some(&posted), DONE)
        {
            return;
        }
        let seq = self.next_seq();
        let mut event = Map::new();
        event.insert("type".into(), Value::String("part".into()));
        event.insert("task_id".into(), Value::String(self.task_id.clone()));
        event.insert("request_id".into(), Value::String(self.request_id.clone()));
        event.insert("part".into(), Value::String(part.id.clone()));
        event.insert("label".into(), Value::String(part.label.clone()));
        event.insert("state".into(), Value::String(state.into()));
        event.insert("seq".into(), Value::String(seq));
        if let Some(step) = part.answer_step.as_ref().filter(|x| !x.is_empty()) {
            event.insert("step".into(), Value::String(step.clone()));
        }
        for (k, v) in fields {
            if shown_field(&v) {
                event.insert(k.into(), v);
            }
        }
        self.live.insert(part.id.clone(), event.clone());
        (self.emit)(Value::Object(event));
    }
}

/// parts.py:211 `composed`: the task's answer as the record keeps it, an index of the card (ORCH-535).
pub fn composed(card: &Card, lines: &[String], artifacts: &[String], chars: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    for row in card.rows() {
        let state = row
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut text = first_line(row.get("text"));
        if state == "asks" {
            if let Some(Value::Object(q)) = row.get("question") {
                text = first_line(q.get("text"));
            }
        }
        let counts = DONE.contains(&state.as_str())
            || state == "failed"
            || state == STOPPED
            || state == "asks";
        let mut said = if !text.is_empty() && counts {
            text
        } else {
            NOT_FINISHED.to_string()
        };
        if state == "done"
            && !truthy(row.get("direct"))
            && (truthy(row.get("note")) || !truthy(row.get("verified")))
        {
            said.push_str(&format!(" ({PARTLY_UNCONFIRMED})"));
        }
        out.push(format!(
            "{}: {said}",
            row.get("label").map(s).unwrap_or_default()
        ));
    }
    if !artifacts.is_empty() {
        out.push(format!("{RESULTS}{}", artifacts.join(", ")));
    }
    out.push(card.line());
    out.extend(lines.iter().cloned());
    out.iter()
        .map(|l| clip(l, chars))
        .collect::<Vec<_>>()
        .join("\n")
}

/// parts.py:233 `_first_line`.
fn first_line(text: Option<&Value>) -> String {
    let t = match text {
        Some(v) if ferb_py::value::truthy(v) => s(v),
        _ => String::new(),
    };
    strip(&t)
        .split('\n')
        .map(strip)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

fn clip(line: &str, chars: usize) -> String {
    if line.chars().count() <= chars {
        line.to_string()
    } else {
        shorten(line, chars, false)
    }
}

/// parts.py:242 `done_line`.
pub fn done_line(done: usize, total: usize, stopped: usize) -> String {
    let mut out = format!("{done} of {total} done");
    if stopped > 0 {
        out.push_str(&format!(" \u{b7} {stopped} stopped by you"));
    }
    out
}
