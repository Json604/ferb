//! What a model run can fail with, as the Python classes worker.py raises and reads (worker.py:43-247), and the
//! owner's words for each: `reason`, `couldnt` and the service, account, connection and size lines. Classified by
//! class and errno only (R2), never by what an error's words say.
//!
//! Python lets any exception travel; here one enum holds every class the worker raises, reads or hands back, with
//! the attributes it reads (`runtime`, `network`, `account`, `reason`, `errno`, `filename`, ...). [`Error::class`]
//! is the Python class name and `Display` is `str(e)`: both are written into records (`"<Class>: <words>"`).

use std::fmt;

use ferb_context::pyvalue::{self, Py};
use ferb_core::errors::FerbError;
use ferb_orchestrator::validate::ValidationError;
use ferb_py::value::{char_len, squeeze, strip};
use serde_json::{Map, Value};

/// worker.py:179 `REASON_CHARS`: an exception's own words in the owner's line, at most; the records keep them whole.
pub const REASON_CHARS: usize = 300;
/// worker.py:180 `BUG`.
pub const BUG: &str = "That's a bug in FERB, not something you asked wrong.";
/// worker.py:74 `ACCOUNT_PROBLEMS`: the account facts no model can change (step 8b, ORCH-315).
pub const ACCOUNT_PROBLEMS: [&str; 2] = ["usage_limit", "signed_out"];
/// answer_rules.py `WRONG_CALLS_CAP` (RUN-446): wrong calls per model run, every role.
pub const WRONG_CALLS_CAP: u64 = 10;

/// Whether this Mac has a network route (ferb/sense/network.py `has_route`): `Some(false)` only when it knows there
/// is none. Read only when a failure could be the network's.
pub type Route<'a> = &'a dyn Fn() -> Option<bool>;

/// The redactor over a text (`gateway.redact` on a `str`).
pub type RedactText<'a> = &'a dyn Fn(&str) -> String;

/// Which `ServiceError` class (worker.py:43-61).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    /// `ServiceError` itself.
    Plain,
    /// `ContextOverflow`: the runtime's window-exceeded error, as its adapter marked it.
    Overflow,
    /// `ImageRefused`: the provider refused an image the request carried, as marked (ORCH-524).
    Image,
}

/// worker.py:43 `ServiceError` (a `ValueError`): the model service itself failed; its words are told plainly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceError {
    pub kind: ServiceKind,
    pub message: String,
    pub runtime: String,
    /// A failure reaching the service, which this Mac being offline explains.
    pub network: bool,
    pub account: String,
}

impl ServiceError {
    pub fn new(message: impl Into<String>, runtime: &str, network: bool, account: &str) -> Self {
        ServiceError {
            kind: ServiceKind::Plain,
            message: message.into(),
            runtime: runtime.to_string(),
            network,
            account: account.to_string(),
        }
    }

    pub fn class(&self) -> &'static str {
        match self.kind {
            ServiceKind::Plain => "ServiceError",
            ServiceKind::Overflow => "ContextOverflow",
            ServiceKind::Image => "ImageRefused",
        }
    }
}

/// worker.py:160 `RuntimeViolation`: the AI runtime broke one of FERB's rules, as its adapter reported it.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeViolation {
    /// `"<runtime>: <data>"`, kept whole in the records.
    pub message: String,
    pub runtime: String,
    pub data: Map<String, Value>,
}

impl RuntimeViolation {
    /// worker.py:169 `plain`: what it did, in the owner's words, from the adapter's fact.
    pub fn plain(&self) -> String {
        if let Some(tools) = self
            .data
            .get("unexpected_tools")
            .filter(|v| ferb_py::value::truthy(v))
        {
            let names = match tools {
                Value::Array(items) => items
                    .iter()
                    .map(|t| pyvalue::to_str(&Py::from_value(t)))
                    .collect::<Vec<_>>()
                    .join(", "),
                other => pyvalue::to_str(&Py::from_value(other)),
            };
            return format!("it used tools FERB didn't offer it: {names}");
        }
        if let Some(auth) = self.data.get("auth").filter(|v| ferb_py::value::truthy(v)) {
            return format!(
                "it signed in a way FERB doesn't allow: {}",
                pyvalue::to_str(&Py::from_value(auth))
            );
        }
        self.message.clone()
    }
}

/// Which `AnswerMissing` class (answer_rules.py:57-75; RUN-445, RUN-446).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingKind {
    Missing,
    /// `EmptyAnswer`: no answer twice in one run.
    Empty,
    /// `ToolLoop`: stopped at the wrong-call cap after `count` wrong calls.
    ToolLoop {
        count: u64,
    },
}

/// answer_rules.py `AnswerMissing` (a `FerbError`): a run that ended without an answer by the shared rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnswerMissing {
    pub kind: MissingKind,
    pub message: String,
    pub runtime: String,
}

impl AnswerMissing {
    /// answer_rules.py:69 `ToolLoop(runtime, count)`.
    pub fn tool_loop(runtime: &str, count: u64) -> Self {
        AnswerMissing {
            kind: MissingKind::ToolLoop { count },
            message: format!(
                "the AI service ({runtime}) kept calling tools it wasn't given, or lookups after its limit ({count} such calls), so FERB stopped this call"
            ),
            runtime: runtime.to_string(),
        }
    }
}

/// An `OSError` (or one of its subclasses) as Python holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsError {
    /// `PermissionError`, `FileNotFoundError`, `TimeoutError`, `OSError`, ...
    pub class: String,
    pub errno: Option<i32>,
    pub strerror: Option<String>,
    /// `os.fsdecode(e.filename)`.
    pub filename: Option<String>,
    /// `str(e)`.
    pub message: String,
}

/// Which adapter class an [`AdapterError`] is (the runtimes' own exceptions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterClass {
    /// codex_stream.py:82 `ProtocolError` (a `ValueError`): Codex's own failure, marked.
    Protocol,
    /// codex_stream.py:125 `AccountError` (a `ProtocolError` and a `RuntimeUnavailable`): an account check at start,
    /// fixed categories and a numeric code only.
    Account,
    /// api.py:46 `ProviderError` (an `Exception`, not a `ValueError`): a provider's failure, worded without the key.
    Provider,
}

/// An adapter's own exception as it travelled (raised by `start`, or out of the event stream), with every fact its
/// adapter marked on it, which the worker and the supervisor read by attribute (`getattr(e, "account", "")`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterError {
    pub class: AdapterClass,
    /// `str(e)`.
    pub message: String,
    /// The model's window overflowed (`overflow`; Codex's `contextWindowExceeded`, a provider's documented error).
    pub overflow: bool,
    /// `usage_limit` / `signed_out` / "" (ORCH-315).
    pub account: String,
    /// The provider refused an image the request carried (ORCH-524).
    pub image: bool,
    /// The turn completed with no final answer (RUN-446).
    pub empty: bool,
    /// A provider refusal's HTTP status; 0 for any other failure.
    pub status: u16,
    /// `AccountError.reason`: `auth_required` or `account_unavailable`; "" for the other classes.
    pub reason: String,
    /// `AccountError.code`.
    pub code: Option<i64>,
}

impl AdapterError {
    /// A failure of `class` with no marked facts.
    pub fn new(class: AdapterClass, message: impl Into<String>) -> AdapterError {
        AdapterError {
            class,
            message: message.into(),
            overflow: false,
            account: String::new(),
            image: false,
            empty: false,
            status: 0,
            reason: String::new(),
            code: None,
        }
    }

    pub fn class_name(&self) -> &'static str {
        match self.class {
            AdapterClass::Protocol => "ProtocolError",
            AdapterClass::Account => "AccountError",
            AdapterClass::Provider => "ProviderError",
        }
    }
}

/// Every exception the worker raises, reads or hands back.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// `ServiceError`, `ContextOverflow`, `ImageRefused`.
    Service(ServiceError),
    /// One of FERB's own errors (ferb/core/errors.py); `RuntimeUnavailable` is [`Error::Unavailable`].
    Ferb(FerbError),
    /// `RuntimeUnavailable` (a `ValueError`) with the adapter's `account` and `reason` attributes ("" when unset).
    Unavailable {
        message: String,
        account: String,
        reason: String,
    },
    /// runtimes/catalog.py `SignedOut` (an `Unsupported`): FERB's own sign-in for `runtime` is missing.
    SignedOut { message: String, runtime: String },
    /// `AnswerMissing`, `EmptyAnswer`, `ToolLoop`.
    AnswerMissing(AnswerMissing),
    /// `RuntimeViolation`.
    Violation(RuntimeViolation),
    /// structured.py `InvalidAnswer` (a `ValueError`): the answer is outside the requested form.
    Invalid(String),
    /// pydantic's `ValidationError` (a `ValueError`).
    Validation(ValidationError),
    /// A plain `ValueError` (the run's own words: "runtime finished without an answer").
    Value(String),
    /// worker.py:265 `Stalled`: no sign of life for stall_s; `progressed`: it had produced output first.
    Stalled { progressed: bool },
    /// `OSError` and its subclasses.
    Os(OsError),
    /// Anything else Python would let travel: its class name and `str()`.
    Other { class: String, message: String },
    /// An adapter's own exception (`ProtocolError`, `AccountError`, `ProviderError`) with its marked facts.
    Adapter(AdapterError),
}

/// `ValueError` subclasses an adapter or a decoder may let travel, read by class: Python's own, and the runtimes'
/// (codex_stream.py:82 `ProtocolError`, :125 `AccountError`, schema_shape.py:50 `SchemaTooComplex`).
const VALUE_ERRORS: [&str; 8] = [
    "ValueError",
    "UnicodeError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "JSONDecodeError",
    "ProtocolError",
    "AccountError",
    "SchemaTooComplex",
];

impl Error {
    pub fn value(message: impl Into<String>) -> Self {
        Error::Value(message.into())
    }

    pub fn other(class: &str, message: impl Into<String>) -> Self {
        Error::Other {
            class: class.to_string(),
            message: message.into(),
        }
    }

    /// A pydantic error, or the exception that escaped a validator (a `TypeError` on a malformed legacy shape).
    pub fn validation(e: ValidationError) -> Self {
        match e.raised() {
            Some(r) => Error::other(r.exception, r.message.clone()),
            None => Error::Validation(e),
        }
    }

    /// The Python class name.
    pub fn class(&self) -> String {
        match self {
            Error::Service(s) => s.class().into(),
            Error::Ferb(e) => e.class().into(),
            Error::Unavailable { .. } => "RuntimeUnavailable".into(),
            Error::SignedOut { .. } => "SignedOut".into(),
            Error::AnswerMissing(a) => match a.kind {
                MissingKind::Missing => "AnswerMissing".into(),
                MissingKind::Empty => "EmptyAnswer".into(),
                MissingKind::ToolLoop { .. } => "ToolLoop".into(),
            },
            Error::Violation(_) => "RuntimeViolation".into(),
            Error::Invalid(_) => "InvalidAnswer".into(),
            Error::Validation(_) => "ValidationError".into(),
            Error::Value(_) => "ValueError".into(),
            Error::Stalled { .. } => "Stalled".into(),
            Error::Os(o) => o.class.clone(),
            Error::Other { class, .. } => class.clone(),
            Error::Adapter(a) => a.class_name().into(),
        }
    }

    /// `isinstance(e, RuntimeUnavailable)`: the adapter couldn't start (an `AccountError` is one).
    pub fn is_runtime_unavailable(&self) -> bool {
        matches!(self, Error::Unavailable { .. })
            || matches!(self, Error::Adapter(a) if a.class == AdapterClass::Account)
    }

    /// `getattr(e, "account", "")`: the account fact a class carries (ORCH-315): `usage_limit`, `signed_out` or "".
    pub fn account(&self) -> &str {
        match self {
            Error::Service(s) => &s.account,
            Error::Unavailable { account, .. } => account,
            Error::SignedOut { .. } => "signed_out",
            Error::Adapter(a) => &a.account,
            _ => "",
        }
    }

    /// `getattr(e, "runtime", "")`.
    pub fn runtime(&self) -> &str {
        match self {
            Error::Service(s) => &s.runtime,
            Error::SignedOut { runtime, .. } => runtime,
            Error::AnswerMissing(a) => &a.runtime,
            Error::Violation(v) => &v.runtime,
            _ => "",
        }
    }

    /// `getattr(e, "overflow", False)` (or a `ContextOverflow`): the model's window overflowed.
    pub fn overflow(&self) -> bool {
        match self {
            Error::Service(s) => s.kind == ServiceKind::Overflow,
            Error::Adapter(a) => a.overflow,
            _ => false,
        }
    }

    /// `getattr(e, "image", False)` (or an `ImageRefused`): the provider refused an image (ORCH-524).
    pub fn image(&self) -> bool {
        match self {
            Error::Service(s) => s.kind == ServiceKind::Image,
            Error::Adapter(a) => a.image,
            _ => false,
        }
    }

    /// `isinstance(e, ValueError)`.
    pub fn is_value_error(&self) -> bool {
        match self {
            Error::Service(_)
            | Error::Unavailable { .. }
            | Error::Invalid(_)
            | Error::Validation(_)
            | Error::Value(_) => true,
            Error::Ferb(e) => e.is_value_error(),
            Error::Other { class, .. } => VALUE_ERRORS.contains(&class.as_str()),
            Error::Adapter(a) => a.class != AdapterClass::Provider,
            _ => false,
        }
    }

    /// `isinstance(e, FerbError)`.
    pub fn is_ferb_error(&self) -> bool {
        match self {
            Error::Ferb(e) => e.is_ferb_error(),
            Error::SignedOut { .. } | Error::AnswerMissing(_) => true,
            _ => false,
        }
    }

    /// `"<Class>: <words>"`, as the records keep a failure.
    pub fn failure(&self) -> String {
        format!("{}: {self}", self.class())
    }
}

impl From<FerbError> for Error {
    fn from(e: FerbError) -> Self {
        match e {
            FerbError::RuntimeUnavailable(message) => Error::Unavailable {
                message,
                account: String::new(),
                reason: String::new(),
            },
            other => Error::Ferb(other),
        }
    }
}

impl fmt::Display for Error {
    /// `str(e)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Service(s) => f.write_str(&s.message),
            Error::Ferb(e) => write!(f, "{e}"),
            Error::Unavailable { message, .. }
            | Error::SignedOut { message, .. }
            | Error::Invalid(message)
            | Error::Value(message) => f.write_str(message),
            Error::AnswerMissing(a) => f.write_str(&a.message),
            Error::Violation(v) => f.write_str(&v.message),
            Error::Validation(v) => write!(f, "{v}"),
            Error::Stalled { .. } => f.write_str("no sign of life"),
            Error::Os(o) => f.write_str(&o.message),
            Error::Other { message, .. } => f.write_str(message),
            Error::Adapter(a) => f.write_str(&a.message),
        }
    }
}

impl std::error::Error for Error {}

/// worker.py:183 `_words`: an exception's words for the owner's line: redacted first (a secret cut in half no longer
/// matches), on one line, at most REASON_CHARS characters, never cut mid-word (ORCH-601).
pub fn words(text: &str, redact: Option<RedactText<'_>>) -> String {
    let redacted = match redact {
        Some(r) => r(text),
        None => text.to_string(),
    };
    let text = squeeze(&redacted);
    if char_len(&text) <= REASON_CHARS {
        text
    } else {
        ferb_orchestrator::lines::shorten(&text, REASON_CHARS, true)
    }
}

/// worker.py:190 `_ended`: a sentence with its own end punctuation, else a period.
pub fn ended(text: &str) -> String {
    if text.ends_with(['.', '!', '?', '…']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

/// worker.py:78 `account_problem`: the owner's line for an account fact (ServiceDown), written by code.
pub fn account_problem(runtime: &str, problem: &str) -> String {
    if problem == "usage_limit" {
        format!(
            "{} has reached its usage limit, so I couldn't go on with this.",
            label(runtime)
        )
    } else {
        format!(
            "{} isn't signed in for FERB (or its sign-in was refused), so I couldn't go on with this.",
            label(runtime)
        )
    }
}

/// runtimes/catalog.py:45 `LABELS` / `label`: how the owner knows each runtime.
pub fn label(name: &str) -> String {
    match name {
        "codex" => "Codex",
        "claude" => "Claude",
        "anthropic" => "the Anthropic API",
        "openai" => "the OpenAI API",
        "google" => "the Google API",
        other => other,
    }
    .to_string()
}

/// worker.py:91 `service_problem`: a failure reaching the service while this Mac has no network route says the Mac
/// seems offline, not "try again shortly".
pub fn service_problem(message: &str, runtime: &str, network: bool, route: Route<'_>) -> String {
    if network && route() == Some(false) {
        return format!(
            "This Mac seems to be offline (it has no network route), so I couldn't reach the AI service I use ({runtime}). Ask again once it's back online."
        );
    }
    format!("The AI service I use ({runtime}) reported an error: {message}")
}

/// worker.py:101 `problem_of`.
pub fn problem_of(e: &ServiceError, route: Route<'_>) -> String {
    service_problem(&e.message, &e.runtime, e.network, route)
}

/// worker.py:105 `retry_stall_problem`: a run that did nothing but retry for `stall_s`.
pub fn retry_stall_problem(runtime: &str, message: &str, stall_s: f64, route: Route<'_>) -> String {
    if route() == Some(false) {
        return service_problem(message, runtime, true, &|| Some(false));
    }
    format!(
        "The AI service I use ({runtime}) only kept retrying for {} s, so I stopped this attempt. Its last word: {}.",
        ferb_clock::py::fmt_g(stall_s),
        message.trim_end_matches('.')
    )
}

/// worker.py:116 `connection_lost`: the owner's line the moment a run's connection drops and its runtime retries
/// it itself (fix wave 7). `task`: a task's run carries on; a chat turn with no task yet answers when it's back.
pub fn connection_lost(runtime: &str, message: &str, task: bool, route: Route<'_>) -> String {
    if route() == Some(false) {
        return "Your Mac seems to be offline: I'll carry on when it's back.".to_string();
    }
    let said = if message.is_empty() {
        String::new()
    } else {
        format!(" It said: {}.", message.trim_end_matches('.'))
    };
    let goes_on = if task {
        "it's retrying and the task carries on"
    } else {
        "it's retrying, and I'll answer when it's back"
    };
    format!("Lost the connection to the AI service I use ({runtime}); {goes_on}.{said}")
}

/// worker.py:152 `too_large_problem`: a call FERB didn't send, over the runtime's per-input character cap (L28).
pub fn too_large_problem(chars: u64, max_chars: u64, runtime: &str) -> String {
    let service = if runtime.is_empty() {
        "the AI service I use".to_string()
    } else {
        format!("the AI service I use ({runtime})")
    };
    format!(
        "I didn't send this to {service}: its context is {} characters, over that service's limit of {} characters for one input.",
        thousands(chars),
        thousands(max_chars)
    )
}

/// `f"{n:,}"`.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// worker.py:199 `reason`: why something FERB was doing failed, in plain words for the owner. `redact` runs before
/// the cap; the exact failure stays in the records.
pub fn reason(e: &Error, redact: Option<RedactText<'_>>, route: Route<'_>) -> String {
    let w = |text: &str| words(text, redact);
    let or_class = |text: String| {
        if text.is_empty() {
            e.class()
        } else {
            text
        }
    };
    match e {
        Error::Service(s) => return problem_of(s, route),
        Error::Ferb(FerbError::InputTooLarge {
            chars,
            max_chars,
            runtime,
            ..
        }) => return too_large_problem(*chars, *max_chars, runtime),
        Error::Os(o) if o.class == "PermissionError" || matches!(o.errno, Some(1) | Some(13)) => {
            let said = w(&or_class(
                o.strerror
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| o.message.clone()),
            ));
            return match &o.filename {
                Some(name) => format!("macOS didn't let FERB open {} ({said}).", w(name)),
                None => format!("macOS refused an operation FERB needed ({said})."),
            };
        }
        _ => {}
    }
    if let Error::Ferb(f) = e {
        if (f.is_refused() && !matches!(f, FerbError::NoTarget(_)))
            || matches!(f, FerbError::AuthError(_))
        {
            return ended(&format!(
                "FERB's protection rules stopped it: {}",
                w(&or_class(e.to_string()))
            ));
        }
    }
    if let Error::Violation(v) = e {
        let who = if v.runtime.is_empty() {
            "the AI runtime".to_string()
        } else {
            format!("the AI runtime ({})", v.runtime)
        };
        return ended(&format!(
            "{who} broke one of FERB's rules: {}",
            w(&v.plain())
        ));
    }
    if let Error::Ferb(FerbError::PlanRejected { problems }) = e {
        return ended(&format!(
            "the plan still had problems after one repair: {}",
            w(&problems.join("; "))
        ));
    }
    if let Error::Validation(v) = e {
        let what = match v.errors().first() {
            Some(first) => {
                let at = first
                    .loc
                    .iter()
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .join(".");
                if at.is_empty() {
                    first.msg.clone()
                } else {
                    format!("{} at {at}", first.msg)
                }
            }
            None => v.to_string(),
        };
        return format!(
            "the AI's answer didn't fit the form FERB requires ({}).",
            w(&what)
        );
    }
    if let Error::Invalid(message) = e {
        return format!(
            "the AI's answer didn't fit the form FERB requires ({}).",
            w(message)
        );
    }
    let text = e.to_string();
    if (e.is_ferb_error() || e.is_runtime_unavailable()) && !strip(&text).is_empty() {
        return ended(&w(&text));
    }
    if let Error::Os(o) = e {
        if o.errno.is_some() {
            let said = w(&o
                .strerror
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| o.message.clone()));
            return match &o.filename {
                Some(name) => format!("macOS reported an error for {} ({said}).", w(name)),
                None => format!("macOS reported an error ({said})."),
            };
        }
    }
    let detail = w(&text);
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    format!("something broke inside FERB ({}{detail}). {BUG}", e.class())
}

/// worker.py:241 `couldnt`: "I couldn't <what>: <reason>". The AI service's own sentence, or the call FERB didn't
/// send, is told as it always was.
pub fn couldnt(what: &str, e: &Error, redact: Option<RedactText<'_>>, route: Route<'_>) -> String {
    if matches!(
        e,
        Error::Service(_) | Error::Ferb(FerbError::InputTooLarge { .. })
    ) {
        return reason(e, None, route);
    }
    format!("I couldn't {what}: {}", reason(e, redact, route))
}
