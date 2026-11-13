//! The seams: what these building blocks and the supervisor need from the rest of FERB, as traits handed over at
//! construction (ferb/orchestrator/ports.py, plus what the modules here took from other parts directly in Python).
//! Nothing here decides anything.
//!
//! - ports.py's six ports (`MemoryPort`, `SchedulePort`, `EnvironmentPort`, `DesktopPort`, `BrowserPort`,
//!   `NotifyPort`) and `QuestionRouter`: the supervisor's, ported later on top of these traits.
//! - [`Policy`]: ferb/policy's words and keys these modules read (confirm.py `site_key`, `account_key`, `from_task`,
//!   `from_task_lines`, `reads_as_ferbs_check`, `FROM_AI`; urls.py `parse`).
//! - [`Journal`]: ferb/memory/journal.py's `append` and `of_kind` (the journal sink).
//! - [`Redact`]: the gateway's redactor as a callable (`gateway.redact`).
//! - [`Wrap`]: ferb/sanitize/untrusted.py `wrap_untrusted` (its random id is the caller's).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::Value;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// ferb/policy: the keys and marks these modules read.
#[allow(clippy::wrong_self_convention)] // Python's names, kept so the port reads as the source
pub trait Policy: Send + Sync {
    /// confirm.py:1091 `site_key`.
    fn site_key(&self, value: Option<&str>) -> String;
    /// confirm.py:1085 `account_key`.
    fn account_key(&self, value: Option<&str>) -> String;
    /// urls.py `parse(url).host`, `None` when it doesn't parse.
    fn url_host(&self, url: &str) -> Option<String>;
    /// confirm.py:320 `from_task`.
    fn from_task(&self, text: &str) -> String;
    /// confirm.py:332 `from_task_lines`.
    fn from_task_lines(&self, text: &str) -> String {
        text.split('\n')
            .map(|line| self.from_task(line))
            .collect::<Vec<_>>()
            .join("\n")
    }
    /// confirm.py:337 `reads_as_ferbs_check`.
    fn reads_as_ferbs_check(&self, text: &str) -> bool;
}

/// confirm.py:133 `FROM_AI`.
pub const FROM_AI: &str = "(written by the AI) ";

/// One journal row as ferb/memory/journal.py's `JournalEntry` holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalEntry {
    pub id: String,
    pub at: String,
    pub kind: String,
    pub data: Value,
    pub origin: String,
}

/// ferb/memory/journal.py `Journal`: `append` (redacts, never drops) and `of_kind`.
pub trait Journal {
    /// journal.py:54 `append`; `origin` None: `journal_origin(kind, data)`.
    fn append(&self, kind: &str, data: Value, origin: Option<&str>) -> JournalEntry;
    /// journal.py:90 `of_kind(kind, field, equals)`: oldest first; an empty field reads every row of the kind.
    fn of_kind(&self, kind: &str, field: &str, equals: &str) -> Vec<JournalEntry>;
}

/// A reader of FERB's records by a key (a task id, a tree id).
pub type Lookup<T> = Arc<dyn Fn(&str) -> T + Send + Sync>;

/// A bus publish: (topic, data).
pub type Publish = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// The gateway's redactor (`gateway.redact`): JSON in, JSON out.
pub type Redact = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

/// No redaction (a test's, or a record that is already redacted).
pub fn no_redact() -> Redact {
    Arc::new(|v: &Value| v.clone())
}

/// `wrap_untrusted`: the untrusted-data wrapper around a text.
pub type Wrap = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The real wrapper, with a random id. Python raises when the system has no random bytes; so does this (a panic
/// carries no text of the data).
pub fn wrap_untrusted() -> Wrap {
    Arc::new(|text: &str| {
        ferb_sanitize::untrusted::wrap_untrusted(text).expect("the system gave no random bytes")
    })
}

/// A wrapper with a fixed id (fixtures; Python's tests fix `secrets.token_hex`).
pub fn wrap_with_id(id: &str) -> Wrap {
    let id = id.to_string();
    Arc::new(move |text: &str| ferb_sanitize::untrusted::wrap_untrusted_with_id(text, &id))
}

/// What a registered tool says of itself that these modules read (`getattr(cap, ...)`): the gateway's registry.
pub trait CapFacts: Send + Sync {
    fn scopes(&self) -> Vec<String>;
    /// The identities as their words (`owner`, `ferb`).
    fn identities(&self) -> Vec<String>;
    fn observation(&self) -> bool;
    fn probe_safe(&self) -> bool;
    fn moves_view(&self) -> bool;
    fn on_screen(&self) -> bool;
    fn holds_tab(&self) -> bool;
    fn probe_keys(&self) -> Vec<String>;
    /// `cap.Input.model_validate(args)`: `Ok(the set fields as model_dump(exclude_unset=True))` or `Err(str(e))`.
    fn validate(&self, args: &Value) -> Result<serde_json::Map<String, Value>, String>;
}

// ---------------------------------------------------------------------------------------------------------------
// ports.py: what the supervisor needs (its port comes later; the shapes are fixed here)
// ---------------------------------------------------------------------------------------------------------------

/// ports.py:13 `MemoryPort`.
pub trait MemoryPort: Send + Sync {
    fn memory_candidates<'a>(&'a self, query: &'a str, targets: &'a str) -> BoxFuture<'a, Value>;
    fn memory_push<'a>(&'a self, query: &'a str, targets: &'a str) -> BoxFuture<'a, Value>;
    /// (items, own words, source, sites, elsewhere) -> per item receipt or refusal.
    fn remember<'a>(
        &'a self,
        items: &'a Value,
        own_words: &'a str,
        source: &'a str,
        sites: &'a Value,
        elsewhere: &'a Value,
    ) -> BoxFuture<'a, Value>;
    fn forget_request(&self, item_id: &str, request_id: &str) -> String;
    fn question_answered(&self, answer: &Value) -> String;
    fn settle(&self) -> BoxFuture<'_, ()>;
    fn identity_context(&self) -> String;
    fn identity_pending(&self) -> String;
    fn settle_identity(&self, summary: &str);
    fn on_episode<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, ()>;
    fn typed_behind_memory(&self, text: &str) -> Vec<String>;
}

/// ports.py:33 `SchedulePort`. `timers` is ferb-clock's `Timers`, reached by the daemon directly.
pub trait SchedulePort: Send + Sync {
    fn on_schedule(
        &self,
        spec: &Value,
        words: &str,
        request_id: &str,
        seen: &[String],
        carried: &[String],
    ) -> (String, Option<Value>);
    fn on_schedule_change(&self, change: &Value, words: &str) -> String;
    fn on_reminder_cancel(&self, cancel: &Value, words: &str) -> String;
    fn schedule_live(&self, schedule_id: &str) -> bool;
    fn on_schedule_edit(
        &self,
        change: &Value,
        words: &str,
        request_id: &str,
        seen: &[String],
        carried: &[String],
    ) -> (String, Option<Value>);
    fn schedule_problems(&self, spec: &Value) -> Vec<String>;
    fn schedules_card(&self) -> (String, Value);
    fn last_run_origins(&self) -> Vec<String>;
}

/// ports.py:53 `EnvironmentPort`.
pub trait EnvironmentPort: Send + Sync {
    fn environment<'a>(&'a self, request_id: &'a str) -> BoxFuture<'a, Value>;
    fn live_facts(&self, owner_state: bool) -> String;
    fn availability(&self) -> Value;
    fn apps(&self) -> Vec<(String, String)>;
    fn running_apps(&self) -> Vec<(String, String, i64)>;
    fn shell_process(&self, process_id: &str) -> Option<Value>;
    fn relevant_context<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Value>;
    fn worker_types(&self) -> Vec<String>;
    fn runtime_account(&self, runtime: &str, fact: &str);
    /// (owner last active, last trail event), as ISO texts.
    fn signals(&self) -> (Option<String>, Option<String>);
    fn skills_catalog(&self) -> Vec<Value>;
    fn abilities_catalog(&self) -> Vec<Value>;
}

/// ports.py:74 `DesktopPort`.
pub trait DesktopPort: Send + Sync {
    fn on_task_start<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, Value>;
    fn on_task_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, Value>;
    fn disturbance_context(&self, tree_id: &str) -> String;
}

/// ports.py:84 `BrowserPort`.
pub trait BrowserPort: Send + Sync {
    fn browser_context(&self, task_id: &str) -> String;
    fn task_tabs(&self, task_id: &str) -> Vec<Value>;
    fn on_loop_rest<'a>(&'a self, task_id: &'a str, resting: bool) -> BoxFuture<'a, ()>;
    fn keep_tabs_open(&self, task_id: &str);
    fn browsing(&self, owner_here: bool, request_id: &str) -> Value;
}

/// ports.py:96 `NotifyPort`.
pub trait NotifyPort: Send + Sync {
    fn on_loop_notice<'a>(
        &'a self,
        tree_id: &'a str,
        step_id: &'a str,
        text: &'a str,
        origin: &'a str,
    ) -> BoxFuture<'a, ()>;
    fn on_resumed_outcome<'a>(&'a self, outcome: &'a Value) -> BoxFuture<'a, ()>;
    fn on_conversation_resume(&self);
}

/// A route for a worker's question: (task id, question, options) -> the answer.
pub type Route =
    Arc<dyn Fn(String, String, Vec<String>) -> BoxFuture<'static, Value> + Send + Sync>;

/// ports.py:103 `QuestionRouter`: the gateway holds it from its construction, the supervisor binds its route.
#[derive(Default)]
pub struct QuestionRouter {
    route: Mutex<Option<Route>>,
}

/// ports.py:115: no orchestrator is bound yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unbound;

impl std::fmt::Display for Unbound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no orchestrator is bound to route worker questions")
    }
}

impl QuestionRouter {
    pub fn bind(&self, route: Route) {
        *self.route.lock().unwrap_or_else(|e| e.into_inner()) = Some(route);
    }

    pub async fn call(
        &self,
        task_id: &str,
        question: &str,
        options: Vec<String>,
    ) -> Result<Value, Unbound> {
        let route = self.route.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match route {
            None => Err(Unbound),
            Some(r) => Ok(r(task_id.to_string(), question.to_string(), options).await),
        }
    }
}
