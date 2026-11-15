//! What the daemon writes to its own output, which `service_streams` sends to `~/.ferb/logs/daemon.{out,err}.log`
//! when launchd gave it none (main.py:718). Python's daemon wrote only what reached the interpreter there (a redacted
//! traceback, `redacting_hooks`, main.py:696) and a few lines for a record it couldn't journal; the journal is its
//! record of everything else. The Rust daemon writes the same, and also a line for each step of its start with how
//! long it took, its stop, and each refused browser connection (bounded as the journal's row is), so the log alone
//! says whether and when the daemon came up. Every line passes the daemon's redactor once it exists: a panic's text,
//! like a traceback's, can hold a value FERB read (rule 7).
//!
//! A panic writes its thread, place, redacted message and a backtrace to stderr and is journaled `daemon.unhandled`
//! (Python's `_loop_exception` for a task nobody awaited) through [`set_unhandled`].

use std::io::Write;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use ferb_sanitize::redact::Redactor;
use serde_json::{json, Map, Value};

static REDACTOR: OnceLock<Arc<Redactor>> = OnceLock::new();
type Unhandled = Box<dyn Fn(Map<String, Value>) + Send + Sync>;
static UNHANDLED: OnceLock<Unhandled> = OnceLock::new();
static STARTED: OnceLock<Instant> = OnceLock::new();

/// The process's first moment, for the start's timings (set by the program before anything else).
pub fn mark_process_start() {
    let _ = STARTED.set(Instant::now());
}

/// Milliseconds since the process started (0 when it wasn't marked).
pub fn since_start_ms() -> u128 {
    STARTED.get().map(|t| t.elapsed().as_millis()).unwrap_or(0)
}

/// The daemon's redactor, once built: every later line passes it.
pub fn set_redactor(redactor: Arc<Redactor>) {
    let _ = REDACTOR.set(redactor);
}

/// Where a panic is journaled (`daemon.unhandled`, bounded like Python's), once the journal exists.
pub fn set_unhandled(record: impl Fn(Map<String, Value>) + Send + Sync + 'static) {
    let _ = UNHANDLED.set(Box::new(record));
}

fn redact(text: &str) -> String {
    match REDACTOR.get() {
        Some(r) => r.text(text),
        None => text.to_string(),
    }
}

/// One line, "<local time> FERB: <text>", redacted, to stderr (the daemon's err log).
pub fn line(text: &str) {
    let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{at} FERB: {}", redact(text));
    let _ = err.flush();
}

/// The panic hook: its thread, place and message (redacted) and a backtrace to stderr, then journaled. A hook never
/// panics itself: what it can't redact it withholds.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        let place = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(a panic without a message)".into());
        let text = std::panic::catch_unwind(|| redact(&payload))
            .unwrap_or_else(|_| "(its text withheld)".into());
        let trace = std::backtrace::Backtrace::force_capture().to_string();
        let trace = std::panic::catch_unwind(|| redact(&trace))
            .unwrap_or_else(|_| "(the backtrace withheld)".into());
        {
            let at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let mut err = std::io::stderr().lock();
            let _ = writeln!(
                err,
                "{at} FERB: a panic in thread {thread} at {place}: {text}\n{trace}"
            );
            let _ = err.flush();
        }
        if let Some(record) = UNHANDLED.get() {
            let detail: String = text
                .chars()
                .take(ferb_daemon::shared::INTERNAL_MESSAGE_CHARS)
                .collect();
            let row = json!({"message": "panic", "error": "panic", "detail": detail, "task": thread,
                             "frames": [place]});
            if let Value::Object(row) = row {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| record(row)));
            }
        }
    }));
}
