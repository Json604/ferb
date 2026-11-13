//! Composition pieces of the daemon: the privacy lists into the one Redactor, the published events' consumers in
//! their order, the journal record of every question FERB asks and how it was answered, and today's activity
//! device id. Port of `ferb/daemon/areas/compose.py` (a mixin of the Daemon). Must not interpret requests or select
//! targets.
//!
//! Not here (main.py's composition, a later part): `_build_stores`, `_build_agents`, `_gateway`, `_recovery` and the
//! protection lists' loading in `_load_config` (see the module's notes in the journal).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use ferb_rpc::HandlerError;
use serde_json::{Map, Value};

use super::core::{refusal, Host};
use super::events::Events;

/// compose.py `question_kind`: a question's kind from who asks it and how (structural; never its words).
pub fn question_kind(capability: &str, kind: &str) -> String {
    use ferb_orchestrator::steering::{
        CONFIRM_ASK, CONVERSATION_ASK, DOWNLOADS_ASK, MEMORY_ASKS, RETRY_ASK,
    };
    if SECRET_KINDS.contains(&kind) {
        return "secret".into();
    }
    let fixed = match capability {
        c if c == CONFIRM_ASK => "confirm",
        c if c == RETRY_ASK => "retry",
        c if c == CONVERSATION_ASK => "conversation_scope",
        "plan.ask" | "owner.ask" => "task",
        c if c == DOWNLOADS_ASK => "downloads",
        "browsing.choose" => "browsing",
        "loop.ask" => "loop",
        "runtime.switch" => "runtime_switch",
        c if MEMORY_ASKS.contains(&c) => "memory",
        c => c,
    };
    fixed.to_owned()
}

/// questions.py `SECRET_KINDS`: a hidden input (a code, secret or password).
pub const SECRET_KINDS: [&str; 3] = ["owner_login_code", "owner_admin_password", "owner_secret"];

/// DMN-583: a question's or a typed answer's first line, as the journal keeps it.
pub const ASKED_CHARS: usize = 200;

/// compose.py `_first_line`: a text's first non-empty line (Python's `splitlines` and `strip`), at most
/// [`ASKED_CHARS`] characters.
pub fn first_line(text: Option<&str>) -> String {
    let text = text.unwrap_or("");
    ferb_orchestrator::py::splitlines(text)
        .into_iter()
        .map(ferb_py::value::strip)
        .find(|l| !l.is_empty())
        .map(|l| ferb_py::value::prefix(l, ASKED_CHARS).to_owned())
        .unwrap_or_default()
}

/// One question FERB asked, as `self.state.approvals[id]` holds it (ferb/daemon/questions.py `Approval`): the fields
/// the journal record reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asked {
    pub id: String,
    pub capability: String,
    pub kind: String,
    pub reason: String,
    pub summary: String,
    pub exact: String,
    pub task_id: String,
    pub request_id: String,
    /// "pending", "granted", "denied", "used", "superseded"
    pub state: String,
    /// The option chosen.
    pub choice: Option<String>,
    /// The typed answer (redacted as the store keeps it).
    pub answer: Option<String>,
    /// The kinds of secret FERB hid in the typed answer.
    pub hidden: Vec<String>,
}

/// `self.state.approvals.get(id)`.
pub trait Approvals: Send + Sync {
    fn get(&self, approval_id: &str) -> Option<Asked>;
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

fn text_of(v: Option<&Value>) -> String {
    v.map(ferb_py::value::py_str).unwrap_or_default()
}

/// compose.py `_question_record` and its state (`_asked_kinds`, `_answered_ids`): every question FERB asks the owner,
/// with its kind, and how it was answered, as `question.asked` / `question.answered` journal rows (so "few questions"
/// can be measured; DMN-583 for ORCH-591). The kind is structural, never a reading of its words. A typed answer's
/// first line is kept, except for a hidden input or an answer FERB hid a secret in. Never a payment's picture
/// (GATE-579).
pub struct QuestionRecords {
    approvals: Arc<dyn Approvals>,
    asked_kinds: Mutex<HashMap<String, String>>,
    answered: Mutex<HashSet<String>>,
}

impl QuestionRecords {
    pub fn new(approvals: Arc<dyn Approvals>) -> QuestionRecords {
        QuestionRecords {
            approvals,
            asked_kinds: Mutex::new(HashMap::new()),
            answered: Mutex::new(HashSet::new()),
        }
    }

    /// The kinds recorded at asking and not yet answered (`_asked_kinds`).
    pub fn asked_kinds(&self) -> HashMap<String, String> {
        self.asked_kinds.lock().unwrap().clone()
    }

    /// The consumer of `approval.requested` and `approval.resolved` (`data` already redacted by the events).
    pub fn record(
        &self,
        host: &Host,
        topic: &str,
        data: &Map<String, Value>,
    ) -> Result<(), HandlerError> {
        let Some(ap) = self.approvals.get(&text_of(data.get("approval_id"))) else {
            return Ok(());
        };
        let given = if truthy(data.get("question_kind")) {
            text_of(data.get("question_kind"))
        } else {
            String::new()
        };
        let kind = if given.is_empty() {
            question_kind(&ap.capability, &ap.kind)
        } else {
            given
        };
        if topic == "approval.requested" {
            self.asked_kinds
                .lock()
                .unwrap()
                .insert(ap.id.clone(), kind.clone());
            let asked = first_line(Some(if ap.summary.is_empty() {
                &ap.reason
            } else {
                &ap.summary
            }));
            let task_id = if truthy(data.get("task_id")) {
                data["task_id"].clone()
            } else if !ap.task_id.is_empty() {
                Value::from(ap.task_id.clone())
            } else {
                Value::Null
            };
            let mut row = Map::new();
            row.insert("approval_id".into(), Value::from(ap.id.clone()));
            row.insert("kind".into(), Value::from(kind));
            row.insert("capability".into(), Value::from(ap.capability.clone()));
            row.insert("task_id".into(), task_id);
            row.insert("request_id".into(), Value::from(ap.request_id.clone()));
            if !asked.is_empty() {
                row.insert("question".into(), Value::from(asked));
            }
            let exact = first_line(Some(&ap.exact));
            if !exact.is_empty() {
                row.insert("exact".into(), Value::from(exact));
            }
            return host.journal("question.asked", Value::Object(row), None);
        }
        if ap.state == "pending" || !self.answered.lock().unwrap().insert(ap.id.clone()) {
            return Ok(());
        }
        let answer = ap.answer.as_deref().unwrap_or("");
        let choice = ap.choice.as_deref().unwrap_or("");
        let said = if !SECRET_KINDS.contains(&ap.kind.as_str()) && ap.hidden.is_empty() {
            first_line(Some(answer))
        } else {
            String::new()
        };
        let kind = self
            .asked_kinds
            .lock()
            .unwrap()
            .remove(&ap.id)
            .unwrap_or(kind);
        let mut row = Map::new();
        row.insert("approval_id".into(), Value::from(ap.id.clone()));
        row.insert("kind".into(), Value::from(kind));
        row.insert("state".into(), Value::from(ap.state.clone()));
        let how = if !choice.is_empty() {
            "option"
        } else if !answer.is_empty() {
            "words"
        } else {
            "none"
        };
        row.insert("answer".into(), Value::from(how));
        if !choice.is_empty() {
            row.insert("choice".into(), Value::from(choice));
        }
        if !said.is_empty() && choice.is_empty() {
            row.insert("said".into(), Value::from(said));
        }
        host.journal("question.answered", Value::Object(row), None)
    }
}

/// compose.py `_publish`: a task's question names its request (the event contract), then the events (redacted
/// once, handed to the consumers).
pub fn publish(events: &Events, approvals: &dyn Approvals, topic: &str, data: Map<String, Value>) {
    let mut data = data;
    if topic == "approval.requested"
        && truthy(data.get("task_id"))
        && !data.contains_key("request_id")
    {
        if let Some(asked) = approvals.get(&text_of(data.get("approval_id"))) {
            if !asked.request_id.is_empty() {
                data.insert("request_id".into(), Value::from(asked.request_id));
            }
        }
    }
    events.publish(topic, &data);
}

/// compose.py `_activity_device`: this Mac's device id as today's activity rows record it (redacted, as the row's
/// other values are), kept until the device id changes.
#[derive(Default)]
pub struct ActivityDevice(Mutex<Option<(String, Value)>>);

impl ActivityDevice {
    pub fn get(&self, host: &Host, device_id: &str) -> Value {
        let mut held = self.0.lock().unwrap();
        match &*held {
            Some((d, v)) if d == device_id => v.clone(),
            _ => {
                let mut m = Map::new();
                m.insert("device_id".into(), Value::from(device_id));
                let v = match host.redact(&Value::Object(m)) {
                    Value::Object(mut o) => o.remove("device_id").unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                *held = Some((device_id.to_owned(), v.clone()));
                v
            }
        }
    }
}

/// What the published events' consumers hand their data to (the other parts of the daemon).
pub trait Consumers: Send + Sync {
    /// `self.supervisor is not None`.
    fn has_supervisor(&self) -> bool;
    /// `supervisor.notify_plan_event(topic, data)`.
    fn plan_event(&self, topic: &str, data: Map<String, Value>);
    /// `daily_activity.record(row)`.
    fn record_activity(&self, row: Map<String, Value>);
    /// `supervisor.conversations.observe(data)`.
    fn observe_conversation(&self, data: Map<String, Value>);
    /// `self._shutting_down`.
    fn shutting_down(&self) -> bool;
    /// `_attention_arrived(data)`.
    fn attention_arrived(&self, data: Map<String, Value>);
    /// `self.bus.publish(topic, data)`: the bus as it is then.
    fn bus(&self, topic: &str, data: Map<String, Value>);
    /// `_approval_notice(data)`.
    fn approval_notice(&self, data: Map<String, Value>);
    /// `_tell_refused(data)`, spawned (GATE-562).
    fn spawn_tell_refused(&self, data: Map<String, Value>);
    /// `_task_paused(data)`, spawned (ORCH-312).
    fn spawn_task_paused(&self, data: Map<String, Value>);
    /// `_task_resumed(data)`, spawned.
    fn spawn_task_resumed(&self, data: Map<String, Value>);
    /// `self.device_id`.
    fn device_id(&self) -> String;
    /// A consumer's failure (a journal write): Python raises it into `publish`'s caller.
    fn failed(&self, _topic: &str, _error: HandlerError) {}
}

/// compose.py `_wire_events`: who hears a published event, in this order, each given the data redacted once.
pub fn wire_events(
    events: &Events,
    host: Host,
    c: Arc<dyn Consumers>,
    questions: Arc<QuestionRecords>,
    device: Arc<ActivityDevice>,
) {
    let x = c.clone();
    events.on(
        None,
        Arc::new(move |topic, data| {
            if x.has_supervisor() {
                x.plan_event(topic, data);
            }
        }),
    );
    let (x, h) = (c.clone(), host.clone());
    events.on(
        Some(&["trail.event", "action.observed"][..]),
        Arc::new(move |topic, data| {
            // SENSE-1: accessibility rows stay in the trail only (pull-only, trail.query)
            if data.get("source") == Some(&Value::from("accessibility")) {
                return;
            }
            let what = match data.get("what") {
                Some(w) => w.clone(),
                None => data
                    .get("source")
                    .cloned()
                    .unwrap_or_else(|| Value::from(topic)),
            };
            let mut row = data;
            row.insert("what".into(), what);
            row.insert("device_id".into(), device.get(&h, &x.device_id()));
            x.record_activity(row);
        }),
    );
    let x = c.clone();
    events.on(
        Some(&["trail.event"][..]),
        Arc::new(move |_, data| {
            if x.has_supervisor() {
                x.observe_conversation(data);
            }
        }),
    );
    let x = c.clone();
    events.on(
        Some(&["perception.attention"][..]),
        Arc::new(move |_, data| {
            if !x.shutting_down() {
                x.attention_arrived(data);
            }
        }),
    );
    let x = c.clone();
    events.on(None, Arc::new(move |topic, data| x.bus(topic, data)));
    let x = c.clone();
    events.on(
        Some(&["approval.requested"][..]),
        Arc::new(move |_, data| x.approval_notice(data)),
    );
    let x = c.clone();
    events.on(
        Some(&["secret.refused"][..]),
        Arc::new(move |_, data| x.spawn_tell_refused(data)),
    );
    let (x, h) = (c.clone(), host);
    events.on(
        Some(&["approval.requested", "approval.resolved"][..]),
        Arc::new(move |topic, data| {
            if let Err(e) = questions.record(&h, topic, &data) {
                x.failed(topic, e);
            }
        }),
    );
    // ORCH-312: a pause stops the task's terminal commands and listening, and a scheduled run's pause pauses its
    // schedule (resumed with it)
    events.on(
        Some(&["task.paused", "task.resumed"][..]),
        Arc::new(move |topic, data| {
            if topic == "task.paused" {
                c.spawn_task_paused(data)
            } else {
                c.spawn_task_resumed(data)
            }
        }),
    );
}

/// compose.py `_apply_privacy`: `[privacy] exclude_domains` / `exclude_apps` into the one Redactor everything holds
/// (the journal, spans, gateway, the shell's egress proxy and hearing read this instance, so a change applies to them
/// at once). `overridden`: a test's own redactor is in place (`_redact_override`): nothing changes.
pub fn apply_privacy(host: &Host, overridden: bool) -> Result<(), HandlerError> {
    if overridden {
        return Ok(());
    }
    let domains: Vec<String> = match host.get("privacy", "exclude_domains")? {
        None => Vec::new(),
        Some(Value::Array(a)) => a.iter().map(ferb_py::value::py_str).collect(),
        Some(_) => return Err(refusal("[privacy] exclude_domains is not a list")),
    };
    let apps = host
        .config
        .exclude_apps()
        .map_err(|e| refusal(e.to_string()))?;
    let domains: Vec<&str> = domains.iter().map(String::as_str).collect();
    let apps: Vec<&str> = apps.iter().map(String::as_str).collect();
    host.redactor.configure(&domains, &apps);
    Ok(())
}

/// compose.py `_load_config`, without the protection lists (main.py's composition loads them once, before the
/// migrations and browsers): this Mac's device id, then the privacy lists into the redactor.
pub fn load_config(host: &Host, overridden: bool) -> Result<String, HandlerError> {
    let device = host
        .get("device", "id")?
        .map(|v| ferb_py::value::py_str(&v))
        .unwrap_or_else(|| "unknown-device".into());
    apply_privacy(host, overridden)?;
    Ok(device)
}
