//! Port of `ferb/clock/tools.py`: the `time.*` tools, reminders for the owner and a worker's durable sleep.
//!
//! A reminder only reminds: it never starts work (scheduled tasks are separate). A sleep counts as waiting (not a
//! stall) and ends with the task. The timer store comes in through [`TimerBook`] (the store is Mac-side), the
//! daemon's sleep through [`Sleeper`].

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, ToolContext, ToolResult};
use serde_json::{json, Map, Value};

use crate::py::{self, Time};
use crate::when::{self, When, WHEN_HELP};
use crate::zone::Zone;

pub const REMIND_AT: &str = "time.remind_at";
pub const SET_TIMER: &str = "time.set_timer";
pub const SLEEP_UNTIL: &str = "time.sleep_until";

/// Each tool's description, as the model reads it.
pub fn description(tool: &str) -> String {
    match tool {
        REMIND_AT => format!("Remind the owner at a time. It only reminds. {WHEN_HELP}"),
        SET_TIMER => "Set a timer for the owner that goes off after some minutes. It only notifies.".into(),
        SLEEP_UNTIL => format!(
            "Pause this task until a time, e.g. to wait for something to be ready. Within the task's time limit; \
             waiting doesn't count as being stuck. Say in why what you are waiting for, in a few words the owner \
             reads: while it waits, the task's status shows the time it wakes and that reason. {WHEN_HELP}"
        ),
        _ => String::new(),
    }
}

/// `describe`: a reminder or timer is a `timer` action with its message; a sleep only reads.
pub fn describe(tool: &str, args: &Value) -> ActionSpec {
    if tool == SLEEP_UNTIL {
        return ActionSpec::new("read");
    }
    let mut spec = ActionSpec::new("timer");
    spec.content = args
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string);
    spec
}

/// The timer store as the tools use it: a new timer's id and its time, or the store's refusal.
pub trait TimerBook {
    fn add_timer(
        &self,
        kind: &str,
        at: &Time,
        message: &str,
        task_id: &str,
        request_id: &str,
    ) -> Result<(String, Time), String>;
}

/// The daemon's durable sleep (`_sleep_until`): returns when its timer fires or the task ends.
pub trait Sleeper: Send + Sync {
    fn sleep<'a>(
        &'a self,
        worker: &'a str,
        task: &'a str,
        at: &'a Time,
        span: Option<&'a str>,
    ) -> BoxFuture<'a, ()>;
}

fn err(msg: impl Into<String>) -> ToolResult {
    let mut content = Map::new();
    content.insert("error".into(), Value::String(msg.into()));
    ToolResult {
        content,
        is_error: true,
        ..ToolResult::default()
    }
}

fn add(
    book: Option<&dyn TimerBook>,
    at: &Time,
    message: &str,
    ctx: &ToolContext,
    kind: &str,
    zone: &Zone,
) -> ToolResult {
    let Some(book) = book else {
        return err("timers aren't available");
    };
    match book.add_timer(kind, at, message, &ctx.task_id, &ctx.request_id) {
        Err(e) => err(e),
        Ok((id, fire_at)) => {
            let mut content = Map::new();
            content.insert("timer".into(), json!(id));
            content.insert(
                "at".into(),
                json!(py::isoformat(&zone.local(&fire_at), py::Spec::Minutes)),
            );
            content.insert(
                "note".into(),
                json!("the owner gets it as a notification and in FERB's chat"),
            );
            ToolResult {
                content,
                ..ToolResult::default()
            }
        }
    }
}

/// `time.remind_at` (args: the `When` fields and `message`).
pub fn remind_at(
    book: Option<&dyn TimerBook>,
    args: &Value,
    now: &Time,
    zone: &Zone,
    ctx: &ToolContext,
) -> ToolResult {
    let at = match when::resolve(&When::from_json(args), now, zone) {
        Ok(at) => at,
        Err(e) => return err(e.message()),
    };
    let message = args
        .get("message")
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    add(book, &at, &message, ctx, "reminder", zone)
}

/// `time.set_timer` (args: `minutes` > 0, `message`).
pub fn set_timer(
    book: Option<&dyn TimerBook>,
    args: &Value,
    now: &Time,
    zone: &Zone,
    ctx: &ToolContext,
) -> ToolResult {
    let minutes = args.get("minutes").and_then(Value::as_f64).unwrap_or(0.0);
    let at = match py::timedelta_us(minutes, py::US_PER_MINUTE).and_then(|us| py::plus_us(now, us))
    {
        Ok(at) => at,
        Err(e) => return err(e.0),
    };
    let message = args
        .get("message")
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    add(book, &at, &message, ctx, "timer", zone)
}

/// `time.sleep_until` (args: the `When` fields). `now` is read again after the sleep; `limit`: the task's own.
pub async fn sleep_until(
    sleeper: Option<&dyn Sleeper>,
    args: &Value,
    now: &dyn Fn() -> Time,
    limit: Option<Time>,
    zone: &Zone,
    ctx: &ToolContext,
) -> ToolResult {
    let at = match when::resolve(&When::from_json(args), &now(), zone) {
        Ok(at) => at,
        Err(e) => return err(e.message()),
    };
    if let Some(l) = limit {
        if at >= l {
            return err(format!(
                "that's past this task's time limit ({})",
                py::fmt(&zone.local(&l), "%H:%M")
            ));
        }
    }
    let Some(sleeper) = sleeper else {
        return err("timers aren't available");
    };
    sleeper
        .sleep(&ctx.worker_id, &ctx.task_id, &at, ctx.span_id.as_deref())
        .await;
    let mut content = Map::new();
    content.insert(
        "woke_at".into(),
        json!(py::isoformat(&zone.local(&now()), py::Spec::Seconds)),
    );
    ToolResult {
        content,
        ..ToolResult::default()
    }
}
