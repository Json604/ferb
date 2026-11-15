//! The answer models (ferb/orchestrator/schemas.py, plans.py, conversations.py, probes.py, ferb/clock/when.py):
//! the pydantic classes FERB validates a model's answer and its own saved records with, as Rust structs. Field order
//! is the classes' declaration order (model_dump writes it). Validation, with pydantic's coercions, validator order
//! and error text, is in [`crate::validate`]; the dumps are there too.
//!
//! `Any` and `dict[str, Any]` fields hold a [`Py`] (Python's value after `json.loads`); an `int` holds a [`PyInt`],
//! since Python's has no size.

use std::fmt;

use ferb_context::Py;

/// A Python `int` as its decimal text (an optional "-", digits, no leading zero), of any size.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct PyInt(pub String);

impl PyInt {
    pub fn from_i64(value: i64) -> PyInt {
        PyInt(value.to_string())
    }

    /// The value when it fits in an `i128` (anything a model writes for a real index or count).
    pub fn value(&self) -> Option<i128> {
        self.0.parse().ok()
    }

    /// The value as an `i64` when it fits.
    pub fn as_i64(&self) -> Option<i64> {
        self.0.parse().ok()
    }

    /// Python's ordering of two ints, whatever their size.
    pub fn cmp_int(&self, other: &PyInt) -> std::cmp::Ordering {
        let (a, b) = (&self.0, &other.0);
        let (na, nb) = (a.starts_with('-'), b.starts_with('-'));
        match (na, nb) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                let (da, db) = (a.trim_start_matches('-'), b.trim_start_matches('-'));
                let mag = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
                if na {
                    mag.reverse()
                } else {
                    mag
                }
            }
        }
    }

    /// `0 <= self < bound`.
    pub fn in_range(&self, bound: usize) -> bool {
        !self.0.starts_with('-') && self.value().is_some_and(|v| v < bound as i128)
    }
}

impl fmt::Display for PyInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// ferb/clock/when.py, ferb/orchestrator/probes.py, conversations.py, plans.py
// ---------------------------------------------------------------------------------------------------------------

/// when.py:36 `When`: every field optional, extra keys ignored.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct When {
    pub in_minutes: Option<f64>,
    pub in_days: Option<PyInt>,
    pub day: Option<String>,
    pub date: Option<String>,
    pub time: Option<String>,
}

/// probes.py:20 `Probe`.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub tool: String,
    pub args_json: String,
    pub field: String,
    /// `equals` | `not_equals` | `changed`.
    pub op: String,
    pub value_json: String,
}

/// conversations.py:14 `ConversationScope`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationScope {
    /// `owner` | `ferb`.
    pub identity: String,
    pub account: String,
    pub site: String,
    pub app: String,
    pub thread: String,
    pub recipients: Vec<String>,
    pub topic: String,
    pub handover_when: String,
    pub window_id: Option<String>,
}

/// plans.py:18 `EventMatch`.
#[derive(Debug, Clone, PartialEq)]
pub struct EventMatch {
    pub field: String,
    pub value: String,
}

/// plans.py:23 `Repeat`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Repeat {
    pub end_when: String,
    pub event: Option<String>,
    pub event_match: Vec<EventMatch>,
}

/// plans.py:35 `Step`.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: String,
    /// `do` | `wait` | `ask` | `converse`.
    pub kind: String,
    pub label: String,
    pub goal: String,
    pub depends_on: Vec<String>,
    pub inputs: Vec<String>,
    pub worker_type: String,
    pub conditions: Vec<String>,
    pub until: Option<String>,
    pub event: Option<String>,
    pub event_match: Vec<EventMatch>,
    pub conversation: Option<ConversationScope>,
    pub question: String,
    pub options: Vec<String>,
    pub repeat: Option<Repeat>,
    pub earlier_results: Vec<String>,
}

impl Step {
    /// A step with the class's defaults: `Step(id=..., kind=...)`.
    pub fn new(id: &str, kind: &str) -> Step {
        Step {
            id: id.to_string(),
            kind: kind.to_string(),
            label: String::new(),
            goal: String::new(),
            depends_on: Vec::new(),
            inputs: Vec::new(),
            worker_type: "default".to_string(),
            conditions: Vec::new(),
            until: None,
            event: None,
            event_match: Vec::new(),
            conversation: None,
            question: String::new(),
            options: Vec::new(),
            repeat: None,
            earlier_results: Vec::new(),
        }
    }

    /// plans.py:77 `loops`: a `do` step with a repeat rule (L31).
    pub fn loops(&self) -> bool {
        self.kind == "do" && self.repeat.is_some()
    }

    /// plans.py:82 `end_when`.
    pub fn end_when(&self) -> &str {
        self.repeat
            .as_ref()
            .map(|r| r.end_when.as_str())
            .unwrap_or("")
    }

    /// plans.py:87 `wakes_on`.
    pub fn wakes_on(&self) -> Option<&str> {
        match &self.repeat {
            Some(r) => r.event.as_deref(),
            None => self.event.as_deref(),
        }
    }

    /// plans.py:92 `wakes_match`.
    pub fn wakes_match(&self) -> &[EventMatch] {
        match &self.repeat {
            Some(r) => &r.event_match,
            None => &self.event_match,
        }
    }
}

/// plans.py:96 `SendScope`.
#[derive(Debug, Clone, PartialEq)]
pub struct SendScope {
    pub capability: String,
    /// `owner` | `ferb`.
    pub identity: String,
    pub account: Option<String>,
    pub site: Option<String>,
    pub recipients: Vec<String>,
    /// `None` is the owner's explicit approval of any body (the default is "").
    pub content: Option<String>,
}

/// plans.py:112 `Part`.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub id: String,
    pub quote: String,
    pub label: String,
    pub direct: Option<String>,
    pub answer_step: Option<String>,
    pub conditions: Vec<PyInt>,
}

/// plans.py:127 `Plan`.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub input_paths: Vec<String>,
    pub big_job: bool,
    pub time_estimate: String,
    pub cost_estimate: String,
    pub standing_sends: Vec<SendScope>,
}

impl Plan {
    pub fn of(steps: Vec<Step>) -> Plan {
        Plan {
            steps,
            input_paths: Vec::new(),
            big_job: false,
            time_estimate: String::new(),
            cost_estimate: String::new(),
            standing_sends: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// ferb/orchestrator/schemas.py
// ---------------------------------------------------------------------------------------------------------------

/// schemas.py:347 `TargetsSpec`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TargetsSpec {
    pub apps: Vec<String>,
    pub sites: Vec<String>,
    pub paths: Vec<String>,
    pub recipients: Vec<String>,
}

/// schemas.py:357 `Check`.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub kind: String,
    pub path: Option<String>,
    pub text: Option<String>,
    pub url_contains: Option<String>,
    pub site: Option<String>,
    pub expected: Option<String>,
    pub tool: Option<String>,
}

impl Check {
    /// schemas.py:377 `NO_CHECK`.
    pub fn none() -> Check {
        Check {
            kind: "none".into(),
            path: None,
            text: None,
            url_contains: None,
            site: None,
            expected: None,
            tool: None,
        }
    }
}

/// schemas.py:381 `Condition`.
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub text: String,
    pub owner_text: String,
    pub check: Check,
    /// `outcome` | `intake` | `artifact` | `external`.
    pub layer: String,
}

/// schemas.py:397 `TaskSpec`.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskSpec {
    pub goal: String,
    pub name: String,
    /// `owner` | `ferb`.
    pub identity: String,
    pub worker_type: String,
    pub targets: TargetsSpec,
    pub success_conditions: Vec<Condition>,
    pub plan: Option<Plan>,
    pub folders: Vec<String>,
    pub keep_tabs_open: bool,
    pub parts: Vec<Part>,
    pub earlier_results: Vec<String>,
}

/// schemas.py:412 `Until`.
#[derive(Debug, Clone, PartialEq)]
pub struct Until {
    pub quote: String,
    pub day: Option<String>,
    pub date: Option<String>,
    pub in_days: Option<PyInt>,
}

/// schemas.py:420 `ScheduleUntil(Until)`: Until's fields first.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleUntil {
    pub quote: String,
    pub day: Option<String>,
    pub date: Option<String>,
    pub in_days: Option<PyInt>,
    pub time: Option<String>,
    pub in_minutes: Option<f64>,
}

impl ScheduleUntil {
    /// schemas.py:434 `when`.
    pub fn when(&self) -> When {
        When {
            in_minutes: self.in_minutes,
            in_days: self.in_days.clone(),
            day: self.day.clone(),
            date: self.date.clone(),
            time: self.time.clone(),
        }
    }
}

/// schemas.py:438 `Remember`.
#[derive(Debug, Clone, PartialEq)]
pub struct Remember {
    pub kind: String,
    pub subject: String,
    pub key: String,
    pub content: String,
    pub quote: String,
    pub login_origin: Option<String>,
    pub until: Option<Until>,
    pub replaces: Vec<String>,
    pub review_in_days: PyInt,
}

/// schemas.py:451 `ForgetRequest`.
#[derive(Debug, Clone, PartialEq)]
pub struct ForgetRequest {
    pub item_id: String,
    pub quote: String,
}

/// schemas.py:457 `AnswerQuestion`.
#[derive(Debug, Clone, PartialEq)]
pub struct AnswerQuestion {
    pub approval_id: String,
    pub answer: String,
}

/// `Literal["unchanged"] | list[SendScope]`.
#[derive(Debug, Clone, PartialEq)]
pub enum StandingSends {
    Unchanged,
    Scopes(Vec<SendScope>),
}

/// schemas.py:465 `TaskControl`.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskControl {
    pub task_id: String,
    /// `stop` | `retry` | `continue` | `pause`.
    pub action: String,
    pub new_goal: bool,
    pub standing_sends: StandingSends,
    pub supersedes: Vec<String>,
}

/// schemas.py:548 `RandomTimes`.
#[derive(Debug, Clone, PartialEq)]
pub struct RandomTimes {
    pub count: PyInt,
    pub start: String,
    pub end: String,
}

/// schemas.py:555 `ScheduleTiming`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleTiming {
    pub repeat: String,
    pub times: Vec<String>,
    pub every_minutes: PyInt,
    pub weekday: String,
    pub random: Vec<RandomTimes>,
}

impl Default for ScheduleTiming {
    fn default() -> ScheduleTiming {
        ScheduleTiming {
            repeat: String::new(),
            times: Vec::new(),
            every_minutes: PyInt::from_i64(0),
            weekday: String::new(),
            random: Vec::new(),
        }
    }
}

/// schemas.py:565 `ScheduleRequest`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleRequest {
    pub request: String,
    pub name: String,
    pub when: When,
    pub repeat: String,
    pub weekday: Option<String>,
    pub every_minutes: Option<PyInt>,
    pub times: Vec<String>,
    pub random: Vec<RandomTimes>,
    pub quote: String,
    pub targets: TargetsSpec,
    pub standing_sends: Vec<SendScope>,
    pub watch: String,
    pub until: Option<ScheduleUntil>,
    pub when_active: bool,
    pub folders: Vec<String>,
    pub identity: Option<String>,
    pub worker_type: String,
    pub profile: String,
    pub account: String,
}

/// schemas.py:596 `ScheduleChange`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleChange {
    pub id: String,
    pub action: String,
    pub quote: String,
    pub request: String,
    pub timing: ScheduleTiming,
    pub until: Option<ScheduleUntil>,
    pub name: String,
}

/// schemas.py:606 `ReminderCancel`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReminderCancel {
    pub id: String,
    pub quote: String,
}

/// schemas.py:612 `TimerRequest`.
#[derive(Debug, Clone, PartialEq)]
pub struct TimerRequest {
    pub when: When,
    pub message: String,
    pub quote: String,
    pub when_active: bool,
    pub name: String,
    /// `reminder` | `timer`.
    pub kind: String,
}

/// schemas.py:623 `ConversationChange`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationChange {
    pub id: String,
    pub action: String,
    pub quote: String,
}

/// schemas.py:480 `SupervisorOutcome`.
#[derive(Debug, Clone, PartialEq)]
pub struct SupervisorOutcome {
    pub control: Option<TaskControl>,
    /// `reply` | `clarify` | `task`.
    pub kind: String,
    pub text: String,
    pub answers_question: bool,
    pub answer_question: Option<AnswerQuestion>,
    pub task: Option<TaskSpec>,
    pub remember: Vec<Remember>,
    pub forget: Vec<ForgetRequest>,
    pub timers: Vec<TimerRequest>,
    pub schedules: Vec<ScheduleRequest>,
    pub schedule_changes: Vec<ScheduleChange>,
    pub show_schedules: bool,
    pub reminder_cancels: Vec<ReminderCancel>,
    pub conversation_changes: Vec<ConversationChange>,
}

impl SupervisorOutcome {
    /// schemas.py:531 `new_task` (ORCH-638): a task that is new work.
    pub fn new_task(&self) -> Option<&TaskSpec> {
        if self.kind != "task" {
            return None;
        }
        let task = self.task.as_ref()?;
        match &self.control {
            None => Some(task),
            Some(c) if c.action == "stop" || c.action == "pause" => Some(task),
            Some(_) => None,
        }
    }
}

/// schemas.py:632 `WorkerReport`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerReport {
    /// `done` | `not_done`.
    pub status: String,
    pub reason: String,
    pub question: Option<String>,
    pub answer: String,
    pub artifacts: Vec<String>,
    pub how: String,
    /// `list[dict[str, Any]]`: each a `Py::Dict`.
    pub input_notes: Vec<Py>,
    pub conversation_action: Option<String>,
    pub conversation_proposals: Vec<Py>,
    pub watch_met: Option<bool>,
    pub loop_action: Option<String>,
    pub probe: Option<Probe>,
    pub part_updates: Vec<Py>,
}

/// schemas.py:659 `QuestionOutcome`.
#[derive(Debug, Clone, PartialEq)]
pub struct QuestionOutcome {
    pub ask_owner: bool,
    pub answer: String,
    pub question: String,
}

/// schemas.py:665 `CheckerResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckerResult {
    pub index: PyInt,
    pub met: Option<bool>,
    pub applies: bool,
    pub evidence: String,
    pub how: Option<String>,
}

/// schemas.py:675 `Finding`.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub quote: String,
    /// `contradicted` | `unsupported`.
    pub kind: String,
    pub evidence: String,
}

/// schemas.py:682 `FixOutcome`.
#[derive(Debug, Clone, PartialEq)]
pub struct FixOutcome {
    pub answer: String,
}

/// schemas.py:686 `RecheckOutcome`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RecheckOutcome {
    pub findings: Vec<Finding>,
}

/// schemas.py:336 `SummaryOutcome`.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryOutcome {
    pub summary: String,
}

/// schemas.py:690 `CheckerOutcome`.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckerOutcome {
    pub results: Vec<CheckerResult>,
    pub next: String,
    pub findings: Vec<Finding>,
    pub missing: String,
    pub instructions: String,
    pub worker_type: Option<String>,
    pub question: String,
    pub reason: String,
    pub dropped_edits: Vec<String>,
}
